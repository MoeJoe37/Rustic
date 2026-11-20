//! NET-01 transport tests over the in-process loopback fixture server.

use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::time::{Duration, Instant};

use rustic_net::fixture::{FixtureServer, Reply};
use rustic_net::{
    Exchange, HttpVersion, NetEvent, NetworkPolicy, ProtocolError, Request, TimeoutKind, Transport,
    TransportError, TransportLimits,
};
use rustic_url::Url;

/// Generous bound for "promptly" on a loaded CI machine.
const PROMPT: Duration = Duration::from_secs(5);

fn transport_for(server: &FixtureServer, limits: TransportLimits) -> Transport {
    let mut policy = NetworkPolicy::deny_all();
    policy.allow(&server.url("/"), server.address()).unwrap();
    Transport::new(policy, limits).unwrap()
}

fn get(transport: &Transport, url: Url) -> Exchange {
    transport.start(Request::get(url).unwrap()).unwrap()
}

/// Drain an exchange and return its terminal error.
fn failure(exchange: Exchange) -> TransportError {
    exchange.collect().unwrap_err()
}

#[test]
fn content_length_response_and_request_serialization() {
    let server = FixtureServer::start([(
        "/page?x=1",
        Reply::response(200, &[("Content-Type", "text/html")], b"<p>hello</p>"),
    )])
    .unwrap();
    let transport = transport_for(&server, TransportLimits::default());
    let mut request = Request::new("POST", server.url("/page?x=1#fragment")).unwrap();
    request.header("Accept", b"text/html").unwrap();
    request.set_body(b"form=1".to_vec());
    let (head, body) = transport.start(request).unwrap().collect().unwrap();
    assert_eq!((head.version(), head.status()), (HttpVersion::Http11, 200));
    assert_eq!(head.headers().get("content-type"), Some(&b"text/html"[..]));
    assert_eq!(body, b"<p>hello</p>");

    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    let sent = &requests[0];
    assert_eq!(
        (sent.method.as_str(), sent.target.as_str()),
        ("POST", "/page?x=1")
    );
    assert_eq!(
        sent.header("host"),
        Some(server.address().to_string().as_str())
    );
    assert_eq!(sent.header("accept"), Some("text/html"));
    assert_eq!(sent.header("content-length"), Some("6"));
    assert_eq!(sent.header("connection"), Some("close"));
    assert_eq!(sent.body, b"form=1");
}

#[test]
fn chunked_close_delimited_and_bodiless_responses() {
    let server = FixtureServer::start([
        (
            "/chunked",
            Reply::new()
                .send("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n")
                .pause(Duration::from_millis(20))
                .send("4;ext=1\r\nabcd\r\n")
                .pause(Duration::from_millis(20))
                .send("2\r\nef\r\n0\r\nTrailer-Field: x\r\n\r\n")
                .hold(),
        ),
        (
            "/close",
            Reply::new().send("HTTP/1.0 200 OK\r\n\r\nuntil the end"),
        ),
        (
            "/empty",
            Reply::new()
                .send("HTTP/1.1 204 No Content\r\nContent-Length: 4\r\n\r\n")
                .hold(),
        ),
        (
            "/interim",
            Reply::new().send(
                "HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 201 Created\r\nContent-Length: 2\r\n\r\nok",
            ),
        ),
    ])
    .unwrap();
    let transport = transport_for(&server, TransportLimits::default());
    // The chunked body ends at its last-chunk even though the server keeps
    // the connection open.
    let (_, body) = get(&transport, server.url("/chunked")).collect().unwrap();
    assert_eq!(body, b"abcdef");
    let (head, body) = get(&transport, server.url("/close")).collect().unwrap();
    assert_eq!(
        (head.version(), body.as_slice()),
        (HttpVersion::Http10, &b"until the end"[..])
    );
    let (head, body) = get(&transport, server.url("/empty")).collect().unwrap();
    assert_eq!((head.status(), body.len()), (204, 0));
    let (head, body) = get(&transport, server.url("/interim")).collect().unwrap();
    assert_eq!((head.status(), body.as_slice()), (201, &b"ok"[..]));
    let (head, body) = transport
        .start(Request::new("HEAD", server.url("/interim")).unwrap())
        .unwrap()
        .collect()
        .unwrap();
    assert_eq!((head.status(), body.len()), (201, 0));
}

#[test]
fn events_arrive_in_order_and_stop_after_the_end() {
    let server = FixtureServer::start([("/", Reply::response(200, &[], b"abc"))]).unwrap();
    let transport = transport_for(&server, TransportLimits::default());
    let exchange = get(&transport, server.url("/"));
    assert!(
        matches!(exchange.next_event(Some(PROMPT)), Some(NetEvent::Response(head)) if head.status() == 200)
    );
    assert_eq!(
        exchange.next_event(Some(PROMPT)),
        Some(NetEvent::Body(b"abc".to_vec()))
    );
    assert_eq!(exchange.next_event(Some(PROMPT)), Some(NetEvent::Finished));
    assert_eq!(exchange.next_event(Some(Duration::ZERO)), None);
}

#[test]
fn origins_outside_the_policy_are_blocked_before_any_connection() {
    let server = FixtureServer::start([("/", Reply::response(200, &[], b"x"))]).unwrap();
    let transport = transport_for(&server, TransportLimits::default());
    let port = server.address().port();
    for url in [
        format!("http://localhost:{port}/"),
        format!("http://127.0.0.1:{}/", port.wrapping_add(1)),
        "http://example.com/".to_owned(),
    ] {
        let request = Request::get(Url::parse(&url, None).unwrap()).unwrap();
        assert_eq!(
            transport.start(request).unwrap_err(),
            TransportError::Blocked,
            "{url}"
        );
    }
    let deny = Transport::new(NetworkPolicy::deny_all(), TransportLimits::default()).unwrap();
    assert_eq!(
        deny.start(Request::get(server.url("/")).unwrap())
            .unwrap_err(),
        TransportError::Blocked
    );
    assert_eq!(server.connections(), 0);
}

#[test]
fn refused_connections_are_reported() {
    // Bind and drop a listener to find a loopback port nobody listens on.
    let address: SocketAddr = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .unwrap()
        .local_addr()
        .unwrap();
    let url = Url::parse(&format!("http://{address}/"), None).unwrap();
    let mut policy = NetworkPolicy::deny_all();
    policy.allow(&url, address).unwrap();
    let transport = Transport::new(policy, TransportLimits::default()).unwrap();
    assert!(matches!(
        failure(get(&transport, url)),
        TransportError::Connect(_)
    ));
}

#[test]
fn idle_timeout_while_waiting_for_the_head_and_the_body() {
    let server = FixtureServer::start([
        ("/silent", Reply::new().hold()),
        (
            "/stall",
            Reply::new()
                .send("HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nabc")
                .hold(),
        ),
    ])
    .unwrap();
    let limits = TransportLimits {
        idle_timeout: Duration::from_millis(150),
        ..TransportLimits::default()
    };
    let transport = transport_for(&server, limits);
    for path in ["/silent", "/stall"] {
        let started = Instant::now();
        assert_eq!(
            failure(get(&transport, server.url(path))),
            TransportError::Timeout(TimeoutKind::Idle)
        );
        let elapsed = started.elapsed();
        assert!(
            elapsed >= Duration::from_millis(150) && elapsed < PROMPT,
            "{path}: {elapsed:?}"
        );
    }
}

#[test]
fn a_dripping_peer_is_not_idle_but_hits_the_total_timeout() {
    let body = vec![b'x'; 200];
    let mut head = b"HTTP/1.1 200 OK\r\nContent-Length: 200\r\n\r\n".to_vec();
    head.extend_from_slice(&body);
    let server =
        FixtureServer::start([("/", Reply::new().drip(&head, 4, Duration::from_millis(20)))])
            .unwrap();
    let limits = TransportLimits {
        idle_timeout: Duration::from_millis(200),
        total_timeout: Some(Duration::from_millis(400)),
        ..TransportLimits::default()
    };
    let transport = transport_for(&server, limits);
    let started = Instant::now();
    let exchange = get(&transport, server.url("/"));
    let mut received = 0;
    let error = loop {
        match exchange.next_event(Some(PROMPT)).expect("an event") {
            NetEvent::Body(bytes) => received += bytes.len(),
            NetEvent::Failed(error) => break error,
            NetEvent::Response(_) => {}
            NetEvent::Finished => panic!("the drip takes about a second"),
        }
    };
    assert_eq!(error, TransportError::Timeout(TimeoutKind::Total));
    assert!(received > 0 && received < 200, "{received}");
    assert!(started.elapsed() < PROMPT);
}

#[test]
fn cancellation_and_drop_stop_the_exchange() {
    let server = FixtureServer::start([(
        "/",
        Reply::new()
            .send("HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\nabc")
            .hold(),
    )])
    .unwrap();
    let transport = transport_for(&server, TransportLimits::default());
    let exchange = get(&transport, server.url("/"));
    assert!(matches!(
        exchange.next_event(Some(PROMPT)),
        Some(NetEvent::Response(_))
    ));
    assert_eq!(
        exchange.next_event(Some(PROMPT)),
        Some(NetEvent::Body(b"abc".to_vec()))
    );
    let cancelled = Instant::now();
    exchange.cancel();
    assert_eq!(
        exchange.next_event(Some(PROMPT)),
        Some(NetEvent::Failed(TransportError::Cancelled))
    );
    assert!(cancelled.elapsed() < PROMPT);
    assert_eq!(exchange.next_event(Some(Duration::ZERO)), None);

    // Dropping an exchange closes its connection: the held server
    // connection sees EOF and the server can still be stopped promptly.
    let dropped = get(&transport, server.url("/"));
    assert!(matches!(
        dropped.next_event(Some(PROMPT)),
        Some(NetEvent::Response(_))
    ));
    drop(dropped);
    let stopping = Instant::now();
    drop(server);
    assert!(stopping.elapsed() < PROMPT);
}

#[test]
fn a_slow_consumer_bounds_buffered_bytes() {
    // 4 MiB through a 64 KiB window: the network thread must stop reading
    // while the consumer is behind, and every byte must arrive intact.
    let body: Vec<u8> = (0..4 * 1024 * 1024u32)
        .map(|index| (index % 251) as u8)
        .collect();
    let server = FixtureServer::start([("/big", Reply::response(200, &[], &body))]).unwrap();
    let limits = TransportLimits {
        buffer_window: 64 * 1024,
        read_chunk: 16 * 1024,
        ..TransportLimits::default()
    };
    let transport = transport_for(&server, limits.clone());
    let exchange = get(&transport, server.url("/big"));
    let mut received = Vec::with_capacity(body.len());
    let mut slow_reads = 0;
    loop {
        match exchange.next_event(Some(PROMPT)).expect("an event") {
            NetEvent::Response(head) => assert_eq!(head.status(), 200),
            NetEvent::Body(bytes) => {
                received.extend_from_slice(&bytes);
                if slow_reads < 20 {
                    // Let the producer fill the window while we are away.
                    std::thread::sleep(Duration::from_millis(10));
                    slow_reads += 1;
                }
            }
            NetEvent::Finished => break,
            NetEvent::Failed(error) => panic!("{error}"),
        }
    }
    assert!(received == body, "body bytes differ");
    let (now, peak) = exchange.buffered_bytes();
    assert_eq!(now, 0);
    // One read beyond a full window can be queued (a piece is never split).
    assert!(
        peak <= limits.buffer_window + limits.read_chunk,
        "peak {peak}"
    );
    assert!(
        peak >= limits.buffer_window / 2,
        "the window was never used: {peak}"
    );
}

#[test]
fn protocol_errors_and_limits_end_the_exchange() {
    let server = FixtureServer::start([
        ("/short", Reply::new().send("HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nabc")),
        ("/nothing", Reply::new()),
        ("/partial-head", Reply::new().send("HTTP/1.1 200 OK\r\nX-A: b")),
        ("/conflict", Reply::new().send("HTTP/1.1 200 OK\r\nContent-Length: 3\r\nContent-Length: 4\r\n\r\nabcd")),
        ("/gzip-te", Reply::new().send("HTTP/1.1 200 OK\r\nTransfer-Encoding: gzip\r\n\r\n")),
        ("/te-wins", Reply::new().send("HTTP/1.1 200 OK\r\nContent-Length: 1\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n0\r\n\r\n")),
        ("/nine", Reply::new().send("<html>HTTP/0.9 body</html>")),
        ("/big", Reply::response(200, &[], &[b'x'; 2000])),
        ("/head", Reply::new().send(format!("HTTP/1.1 200 OK\r\nX: {}\r\n\r\n", "y".repeat(3000)))),
        ("/upgrade", Reply::new().send("HTTP/1.1 101 Switching Protocols\r\n\r\n")),
    ])
    .unwrap();
    let limits = TransportLimits {
        max_body_bytes: 1000,
        max_head_bytes: 2048,
        ..TransportLimits::default()
    };
    let transport = transport_for(&server, limits);
    let protocol = |path: &str| failure(get(&transport, server.url(path)));
    assert_eq!(protocol("/short"), ProtocolError::Truncated.into());
    assert_eq!(protocol("/nothing"), ProtocolError::Empty.into());
    assert_eq!(protocol("/partial-head"), ProtocolError::Truncated.into());
    assert_eq!(
        protocol("/conflict"),
        ProtocolError::InvalidContentLength.into()
    );
    assert_eq!(
        protocol("/gzip-te"),
        ProtocolError::UnsupportedTransferCoding.into()
    );
    assert_eq!(protocol("/nine"), ProtocolError::InvalidStatusLine.into());
    assert_eq!(protocol("/big"), ProtocolError::BodyTooLarge.into());
    assert_eq!(protocol("/head"), ProtocolError::HeadTooLarge.into());
    assert_eq!(
        protocol("/upgrade"),
        ProtocolError::UnexpectedUpgrade.into()
    );
    let (_, body) = get(&transport, server.url("/te-wins")).collect().unwrap();
    assert_eq!(body, b"abc");
}

#[test]
fn concurrent_exchanges_are_independent() {
    let server = FixtureServer::start([
        (
            "/slow",
            Reply::new()
                .pause(Duration::from_millis(200))
                .send("HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\nslow"),
        ),
        ("/fast", Reply::response(200, &[], b"fast")),
    ])
    .unwrap();
    let transport = transport_for(&server, TransportLimits::default());
    let slow = get(&transport, server.url("/slow"));
    let fast = get(&transport, server.url("/fast"));
    let (_, fast_body) = fast.collect().unwrap();
    assert_eq!(fast_body, b"fast");
    assert_eq!(slow.collect().unwrap().1, b"slow");
    assert_eq!(server.connections(), 2);
    let unknown = get(&transport, server.url("/missing")).collect().unwrap();
    assert_eq!(
        (unknown.0.status(), unknown.1.as_slice()),
        (404, &b"not found"[..])
    );
}

#[test]
fn content_codings_are_advertised_and_decoded() {
    let gzip = include_bytes!("fixtures/coding/hello.gz");
    let brotli = include_bytes!("fixtures/coding/hello.br");
    let bomb = include_bytes!("fixtures/coding/zeros-16m.br");
    let chunked_brotli = {
        let mut bytes =
            b"HTTP/1.1 200 OK\r\nContent-Encoding: br\r\nTransfer-Encoding: chunked\r\n\r\n"
                .to_vec();
        for piece in brotli.chunks(5) {
            bytes.extend_from_slice(format!("{:x}\r\n", piece.len()).as_bytes());
            bytes.extend_from_slice(piece);
            bytes.extend_from_slice(b"\r\n");
        }
        bytes.extend_from_slice(b"0\r\n\r\n");
        bytes
    };
    let server = FixtureServer::start([
        (
            "/gzip",
            Reply::response(200, &[("Content-Encoding", "gzip")], gzip),
        ),
        ("/br", Reply::new().send(chunked_brotli)),
        (
            "/zstd",
            Reply::response(200, &[("Content-Encoding", "zstd")], b"raw"),
        ),
        (
            "/bomb",
            Reply::response(200, &[("Content-Encoding", "br")], bomb),
        ),
        (
            "/corrupt",
            Reply::response(200, &[("Content-Encoding", "gzip")], b"not gzip"),
        ),
        (
            "/head",
            Reply::response(200, &[("Content-Encoding", "gzip")], b""),
        ),
    ])
    .unwrap();
    let mut policy = NetworkPolicy::deny_all();
    policy.allow(&server.url("/"), server.address()).unwrap();
    let limits = TransportLimits {
        max_body_bytes: 1 << 20,
        ..TransportLimits::default()
    };
    let transport = Transport::new(policy, limits).unwrap();
    let get = |path: &str| {
        transport
            .start(Request::get(server.url(path)).unwrap())
            .unwrap()
            .collect()
            .map(|(_, body)| body)
    };
    assert_eq!(get("/gzip").unwrap(), b"hello hello hello\n");
    assert_eq!(get("/br").unwrap(), b"hello hello hello\n");
    assert_eq!(get("/zstd").unwrap(), b"raw");
    assert_eq!(
        get("/bomb").unwrap_err(),
        TransportError::Protocol(ProtocolError::BodyTooLarge)
    );
    assert_eq!(
        get("/corrupt").unwrap_err(),
        TransportError::Protocol(ProtocolError::InvalidContentCoding)
    );
    let head = transport
        .start(Request::new("HEAD", server.url("/head")).unwrap())
        .unwrap()
        .collect()
        .unwrap();
    assert!(head.1.is_empty());
    let log = server.requests();
    assert_eq!(log[0].header("accept-encoding"), Some("gzip, deflate, br"));
    let mut ranged = Request::get(server.url("/zstd")).unwrap();
    ranged.header("Range", b"bytes=0-").unwrap();
    transport.start(ranged).unwrap().collect().unwrap();
    assert_eq!(
        server.requests().last().unwrap().header("accept-encoding"),
        None
    );
}

#[test]
fn offline_parsing_is_split_invariant_for_leading_blank_bytes() {
    use rustic_net::{ProtocolError, parse_response};
    let limits = TransportLimits::default();
    let input = b"\r\r\nHTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok";
    let whole = parse_response(&[input], &limits, false);
    assert_eq!(whole.as_ref().map(|(_, body)| body.as_slice()), Ok(&b"ok"[..]));
    let bytes: Vec<&[u8]> = input.chunks(1).collect();
    assert_eq!(parse_response(&bytes, &limits, false), whole);
    // The fuzzer's reproducer: only blank-line bytes, then the end.
    assert_eq!(
        parse_response(&[b"\r", b"\r", b"\n"], &limits, false),
        parse_response(&[b"\r\r\n"], &limits, false)
    );
    assert_eq!(
        parse_response(&[b"\r\r\n"], &limits, false).map(|_| ()),
        Err(ProtocolError::Truncated)
    );
}
