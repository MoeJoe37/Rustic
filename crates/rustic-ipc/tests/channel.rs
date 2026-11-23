//! Channels between real byte streams: round trips, handshakes, hostile
//! frames and peer death.

use std::io::{Cursor, Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};

use rustic_ipc::protocol::{
    AssistiveRequest, BrokerMessage, CompositionPhase, DocumentGrant, FetchRequest, FetchResponse,
    FrameAsk, FrameLayer, FrameRequest, MODIFIER_SHIFT, PageInput, Painted, RendererMessage,
    ResourceBody, ResourceFailure, ResourceKind, Start, StorageArea, StorageOp, StorageValue,
    Surface,
};
use rustic_ipc::{
    Channel, DecodeError, IpcError, Limits, MAGIC, PROTOCOL_VERSION, Role, decode_frame,
    encode_frame,
};

fn pair(limits: Limits) -> (Channel<TcpStream, TcpStream>, Channel<TcpStream, TcpStream>) {
    // A loopback connection: a portable full-duplex byte stream.
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let a = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (b, _) = listener.accept().unwrap();
    let (a2, b2) = (a.try_clone().unwrap(), b.try_clone().unwrap());
    let renderer =
        std::thread::spawn(move || Channel::connect(b, b2, Role::Renderer, limits).unwrap());
    let broker = Channel::connect(a, a2, Role::Broker, limits).unwrap();
    (broker, renderer.join().unwrap())
}

fn samples() -> (Vec<BrokerMessage>, Vec<RendererMessage>) {
    let broker = vec![
        BrokerMessage::Start(Start {
            command: "dump-dom".into(),
            args: vec!["--max-nodes".into(), "9".into()],
            document: DocumentGrant {
                url: "https://a.test/".into(),
                bytes: b"<p>\xff".to_vec(),
                transport_charset: Some("utf-8".into()),
                local: false,
                csp: vec!["default-src 'self'".into()],
                csp_report_only: vec![],
                referrer_policy: "no-referrer".into(),
                storage: true,
            },
            fonts: vec![b"OTTO".to_vec()],
            families: vec!["sans-serif=Arial".into()],
        }),
        BrokerMessage::Resource {
            id: 7,
            result: Ok(ResourceBody {
                bytes: vec![0, 1, 2],
                charset: None,
            }),
        },
        BrokerMessage::Resource {
            id: 8,
            result: Err(ResourceFailure::Blocked),
        },
        BrokerMessage::Fetch {
            id: 9,
            result: Some(FetchResponse {
                kind: "cors".into(),
                url: Some("https://b.test/".into()),
                redirected: true,
                status: 201,
                status_text: b"Created".to_vec(),
                headers: vec![("x".into(), b"1".to_vec())],
                body: b"{}".to_vec(),
            }),
        },
        BrokerMessage::Fetch {
            id: 10,
            result: None,
        },
        BrokerMessage::Storage {
            id: 11,
            value: StorageValue::Keys(vec![vec![0xD800], vec![]]),
        },
        BrokerMessage::Storage {
            id: 12,
            value: StorageValue::Units(None),
        },
        BrokerMessage::Storage {
            id: 13,
            value: StorageValue::QuotaExceeded,
        },
        BrokerMessage::Frame {
            id: 14,
            result: Ok(3),
        },
        BrokerMessage::Frame {
            id: 15,
            result: Err(ResourceFailure::Cancelled),
        },
        BrokerMessage::Popup {
            id: 16,
            opened: true,
        },
        BrokerMessage::Page(PageInput::Frame(FrameAsk {
            width: 800,
            height: 600,
            scale: 1.5,
            zoom: 1.25,
            scroll: Some((0.0, 120.5)),
            elapsed_us: 16_667,
            find: Some(("needle".into(), 3)),
        })),
        BrokerMessage::Page(PageInput::Frame(FrameAsk {
            width: 1,
            height: 1,
            scale: 0.25,
            zoom: 5.0,
            scroll: None,
            elapsed_us: 0,
            find: None,
        })),
        BrokerMessage::Page(PageInput::Click {
            x: 10.5,
            y: -3.0,
            modifiers: MODIFIER_SHIFT,
        }),
        BrokerMessage::Page(PageInput::Key {
            down: true,
            key: "Enter".into(),
            code: "NumpadEnter".into(),
            repeat: false,
            modifiers: 0,
        }),
        BrokerMessage::Page(PageInput::Composition {
            phase: CompositionPhase::Update,
            text: "\u{304b}".into(),
        }),
        BrokerMessage::Page(PageInput::Composition {
            phase: CompositionPhase::Start,
            text: String::new(),
        }),
        BrokerMessage::Page(PageInput::Assistive {
            node: u64::MAX,
            request: AssistiveRequest::SetValue("typed".into()),
        }),
        BrokerMessage::Page(PageInput::Assistive {
            node: 7,
            request: AssistiveRequest::SetSelection {
                anchor: 3,
                focus: 1,
            },
        }),
        BrokerMessage::Page(PageInput::Assistive {
            node: 7,
            request: AssistiveRequest::Click,
        }),
        BrokerMessage::Page(PageInput::Accessibility),
        BrokerMessage::AutomationCommand {
            id: 17,
            line: Some("{\"id\":1,\"method\":\"page.info\"}".into()),
        },
        BrokerMessage::AutomationCommand { id: 18, line: None },
    ];
    let renderer = vec![
        RendererMessage::Resource {
            id: 1,
            url: "https://a.test/s.css".into(),
            kind: ResourceKind::Stylesheet,
            max_bytes: 100,
        },
        RendererMessage::Resource {
            id: 2,
            url: "x".into(),
            kind: ResourceKind::Script {
                nonce: Some("n".into()),
                parser_inserted: false,
                module: true,
            },
            max_bytes: u64::MAX,
        },
        RendererMessage::Fetch {
            id: 3,
            request: FetchRequest {
                method: "POST".into(),
                url: "/api".into(),
                headers: vec![("a".into(), vec![0xff])],
                body: Some(vec![]),
                mode: "cors".into(),
                credentials: "include".into(),
                redirect: "follow".into(),
                referrer_policy: String::new(),
            },
        },
        RendererMessage::Storage {
            id: 4,
            area: StorageArea::Session,
            op: StorageOp::Set(vec![1, 2], vec![0xDC00]),
        },
        RendererMessage::Diagnostic("console.log: hi".into()),
        RendererMessage::Frame {
            id: 5,
            request: FrameRequest {
                url: "about:srcdoc".into(),
                srcdoc: Some("<p>x".into()),
                sandbox: Some(vec!["allow-scripts".into()]),
                width: 300,
                height: 150,
            },
        },
        RendererMessage::Frame {
            id: 6,
            request: FrameRequest::default(),
        },
        RendererMessage::Popup {
            id: 7,
            url: "https://b.test/".into(),
            noopener: true,
            noreferrer: false,
        },
        RendererMessage::AutomationOutput("{\"id\":1,\"result\":null}".into()),
        RendererMessage::AutomationRead { id: 8 },
        RendererMessage::Surface(Surface {
            width: 2,
            height: 1,
            pixels: vec![0, 0, 0, 255, 1, 2, 3, 4],
            layers: vec![FrameLayer {
                handle: 3,
                x: 1,
                y: 0,
                width: 1,
                height: 1,
                coverage: vec![200],
            }],
        }),
        RendererMessage::Painted(Painted {
            surface: Surface {
                width: 1,
                height: 1,
                pixels: vec![1, 2, 3, 255],
                layers: Vec::new(),
            },
            scroll: (0.0, 40.0),
            next_frame_us: Some(16_667),
            found: Some((3, 1, 40.0, 56.0)),
        }),
        RendererMessage::Handled,
        RendererMessage::Accessibility(vec![1, 2, 3]),
        RendererMessage::Done {
            status: 0,
            output: b"#document\n".to_vec(),
        },
    ];
    (broker, renderer)
}

#[test]
fn every_message_round_trips_over_a_stream_pair() {
    let (mut broker, mut renderer) = pair(Limits::default());
    let (to_renderer, to_broker) = samples();
    for message in &to_renderer {
        broker.send(message).unwrap();
        assert_eq!(&renderer.recv::<BrokerMessage>().unwrap(), message);
    }
    for message in &to_broker {
        renderer.send(message).unwrap();
        assert_eq!(&broker.recv::<RendererMessage>().unwrap(), message);
    }
}

#[test]
fn handshakes_must_match_protocol_version_and_role() {
    let hello = |version: u16, role: u8| {
        let mut bytes = MAGIC.to_vec();
        bytes.extend_from_slice(&version.to_le_bytes());
        bytes.push(role);
        bytes
    };
    for (peer, ok) in [
        (hello(PROTOCOL_VERSION, 2), true),
        (hello(PROTOCOL_VERSION + 1, 2), false),
        (hello(PROTOCOL_VERSION, 1), false),
        (b"RUSTICXX\x01\x00\x02".to_vec(), false),
    ] {
        let result = Channel::connect(
            Cursor::new(peer),
            Vec::new(),
            Role::Broker,
            Limits::default(),
        );
        assert_eq!(result.is_ok(), ok);
        if !ok {
            assert!(matches!(result, Err(IpcError::Handshake)));
        }
    }
    let short = Channel::connect(
        Cursor::new(b"RUST".to_vec()),
        Vec::new(),
        Role::Broker,
        Limits::default(),
    );
    assert!(matches!(short, Err(IpcError::Closed)));
}

fn renderer_hello() -> Vec<u8> {
    let mut bytes = MAGIC.to_vec();
    bytes.extend_from_slice(&PROTOCOL_VERSION.to_le_bytes());
    bytes.push(2);
    bytes
}

fn broker_reading(frames: &[u8], limits: Limits) -> Channel<Cursor<Vec<u8>>, Vec<u8>> {
    let mut input = renderer_hello();
    input.extend_from_slice(frames);
    Channel::connect(Cursor::new(input), Vec::new(), Role::Broker, limits).unwrap()
}

#[test]
fn hostile_frames_are_refused_without_allocating_their_claims() {
    let limits = Limits {
        max_frame_bytes: 64,
    };
    // A frame announcing 4 GiB is refused before reading it.
    let mut channel = broker_reading(&u32::MAX.to_le_bytes(), limits);
    assert!(matches!(
        channel.recv::<RendererMessage>(),
        Err(IpcError::FrameTooLarge(4_294_967_295))
    ));
    // A frame cut short by the peer's death is Closed.
    let mut frame = 20u32.to_le_bytes().to_vec();
    frame.extend_from_slice(&[5, 0, 0]);
    let mut channel = broker_reading(&frame, limits);
    assert!(matches!(
        channel.recv::<RendererMessage>(),
        Err(IpcError::Closed)
    ));
    // A clean end between frames is Closed too.
    let mut channel = broker_reading(&[], limits);
    assert!(matches!(
        channel.recv::<RendererMessage>(),
        Err(IpcError::Closed)
    ));
    // Sending an oversized frame fails locally.
    let mut channel = broker_reading(&[], limits);
    let big = BrokerMessage::Resource {
        id: 0,
        result: Ok(ResourceBody {
            bytes: vec![0; 100],
            charset: None,
        }),
    };
    assert!(matches!(
        channel.send(&big),
        Err(IpcError::FrameTooLarge(_))
    ));
}

#[test]
fn malformed_payloads_decode_to_errors() {
    let decode = |bytes: &[u8]| {
        decode_frame::<RendererMessage>(bytes).map_err(|error| match error {
            IpcError::Malformed(error) => error,
            other => panic!("{other}"),
        })
    };
    assert_eq!(decode(&[]), Err(DecodeError::Truncated));
    assert_eq!(decode(&[99, 0]), Err(DecodeError::UnknownTag(99)));
    let mut done = encode_frame(&RendererMessage::Done {
        status: 0,
        output: vec![],
    });
    done.push(0);
    assert_eq!(decode(&done), Err(DecodeError::TrailingBytes));
    // A string with invalid UTF-8.
    let mut diagnostic = vec![4, 0];
    diagnostic.extend_from_slice(&2u32.to_le_bytes());
    diagnostic.extend_from_slice(&[0xC3, 0x28]);
    assert_eq!(decode(&diagnostic), Err(DecodeError::InvalidUtf8));
    // A length past the end of the frame.
    let mut long = vec![4, 0];
    long.extend_from_slice(&1000u32.to_le_bytes());
    assert_eq!(decode(&long), Err(DecodeError::Truncated));
    // Odd-length UTF-16 and a bad boolean.
    let mut storage = vec![3, 0, 0, 0, 0, 0, 0, 2];
    storage.extend_from_slice(&3u32.to_le_bytes());
    storage.extend_from_slice(&[1, 2, 3]);
    assert_eq!(decode(&storage), Err(DecodeError::OutOfRange));
    let mut script = vec![1, 0, 0, 0, 0, 0];
    script.extend_from_slice(&0u32.to_le_bytes());
    script.extend_from_slice(&[2, 0, 7]);
    assert_eq!(decode(&script), Err(DecodeError::InvalidFlag));
    // Too many header fields for the protocol.
    let mut fetch = vec![2, 0, 0, 0, 0, 0];
    for _ in 0..2 {
        fetch.extend_from_slice(&0u32.to_le_bytes());
    }
    fetch.extend_from_slice(&10_000u32.to_le_bytes());
    assert_eq!(decode(&fetch), Err(DecodeError::OutOfRange));
}

#[test]
fn a_child_process_speaks_the_protocol_over_its_standard_streams() {
    // The test binary re-runs itself as the child.
    if std::env::var_os("RUSTIC_IPC_CHILD").is_some() {
        let stdin = std::io::stdin().lock();
        let stdout = std::io::stdout().lock();
        let mut channel =
            Channel::connect(stdin, stdout, Role::Renderer, Limits::default()).unwrap();
        let BrokerMessage::Start(start) = channel.recv::<BrokerMessage>().unwrap() else {
            std::process::exit(3);
        };
        channel
            .send(&RendererMessage::Done {
                status: 0,
                output: start.document.bytes,
            })
            .unwrap();
        std::process::exit(0);
    }
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "a_child_process_speaks_the_protocol_over_its_standard_streams",
            "--nocapture",
        ])
        .env("RUSTIC_IPC_CHILD", "1")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let stdin = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap();
    // The test harness may print before the child's handshake; skip to it.
    let mut skipped = Vec::new();
    let mut byte = [0u8];
    while !skipped.ends_with(&MAGIC[..]) {
        stdout.read_exact(&mut byte).unwrap();
        skipped.push(byte[0]);
    }
    let mut prefix = MAGIC.to_vec();
    prefix.extend_from_slice(&PROTOCOL_VERSION.to_le_bytes());
    prefix.push(2);
    let mut rest = [0u8; 3];
    stdout.read_exact(&mut rest).unwrap();
    let mut replay = prefix[..8].to_vec();
    replay.extend_from_slice(&rest);
    let reader = Cursor::new(replay).chain(stdout);
    let mut channel = Channel::connect(reader, stdin, Role::Broker, Limits::default()).unwrap();
    channel
        .send(&BrokerMessage::Start(Start {
            document: DocumentGrant {
                bytes: b"echo".to_vec(),
                ..DocumentGrant::default()
            },
            ..Start::default()
        }))
        .unwrap();
    assert_eq!(
        channel.recv::<RendererMessage>().unwrap(),
        RendererMessage::Done {
            status: 0,
            output: b"echo".to_vec()
        }
    );
    // The child exits; the channel then reports Closed, not a hang.
    assert!(child.wait().unwrap().success());
    assert!(matches!(
        channel.recv::<RendererMessage>(),
        Err(IpcError::Closed)
    ));
    let _ = std::io::stderr().flush();
}

#[test]
fn page_input_with_bad_numbers_or_long_keys_is_refused() {
    let found = |find: Option<(String, u32)>| {
        encode_frame(&BrokerMessage::Page(PageInput::Frame(FrameAsk {
            width: 10,
            height: 10,
            scale: 1.0,
            zoom: 1.0,
            scroll: None,
            elapsed_us: 0,
            find,
        })))
    };
    let frame = |width: u32, scale: f64, scroll: Option<(f64, f64)>| {
        encode_frame(&BrokerMessage::Page(PageInput::Frame(FrameAsk {
            width,
            height: 10,
            scale,
            zoom: 1.0,
            scroll,
            elapsed_us: 0,
            find: None,
        })))
    };
    assert!(decode_frame::<BrokerMessage>(&found(Some(("a".into(), 0)))).is_ok());
    assert!(decode_frame::<BrokerMessage>(&frame(10, 1.0, Some((0.0, 1.0)))).is_ok());
    for bad in [
        frame(0, 1.0, None),
        frame(16_385, 1.0, None),
        frame(10, 9.0, None),
        frame(10, f64::NAN, None),
        frame(10, 1.0, Some((f64::INFINITY, 0.0))),
        frame(10, 1.0, Some((0.0, f64::NAN))),
        found(Some((String::new(), 0))),
        found(Some(("x".repeat(1025), 0))),
        encode_frame(&BrokerMessage::Page(PageInput::Key {
            down: true,
            key: "x".repeat(65),
            code: String::new(),
            repeat: false,
            modifiers: 0,
        })),
    ] {
        assert!(decode_frame::<BrokerMessage>(&bad).is_err());
    }
    let painted = encode_frame(&RendererMessage::Painted(Painted {
        scroll: (f64::NAN, 0.0),
        ..Painted::default()
    }));
    assert!(decode_frame::<RendererMessage>(&painted).is_err());
}
