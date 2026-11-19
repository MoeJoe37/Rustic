//! Error reporting, rejection events, event handler attributes and console.

use std::cell::{Ref, RefCell, RefMut};
use std::rc::Rc;

use rustic_bindings::dom_exception;
use rustic_bindings::page::{self, ConsoleLevel, ConsoleMessage, PageTree};
use rustic_dom::{Document, DocumentLimits, QualifiedName};
use rustic_js::runtime::Agent;

#[derive(Debug)]
struct Tree(RefCell<Document>);

impl PageTree for Tree {
    fn read(&self) -> Option<Ref<'_, Document>> {
        self.0.try_borrow().ok()
    }

    fn write(&self) -> Option<RefMut<'_, Document>> {
        self.0.try_borrow_mut().ok()
    }
}

fn page() -> Agent {
    let mut document = Document::new(DocumentLimits::default()).unwrap();
    let html = document
        .create_element(QualifiedName::html("html").unwrap())
        .unwrap();
    let body = document
        .create_element(QualifiedName::html("body").unwrap())
        .unwrap();
    document.append_child(document.root(), html).unwrap();
    document.append_child(html, body).unwrap();
    let tree = Rc::new(Tree(RefCell::new(document)));
    let mut agent = Agent::new().unwrap();
    dom_exception::install(&mut agent).unwrap();
    page::install(&mut agent, tree).unwrap();
    agent.heap.set_gc_stress(true);
    agent
}

/// A microtask checkpoint followed by rejection notification, as a host runs.
fn checkpoint(agent: &mut Agent) {
    loop {
        page::flush_mutation_records(agent).unwrap();
        agent.run_jobs().unwrap();
        let events = agent.take_rejection_events();
        if events.is_empty() {
            break;
        }
        page::report_rejections(agent, events).unwrap();
    }
}

fn messages(agent: &Agent) -> Vec<ConsoleMessage> {
    let log = page::take_console_messages(agent);
    assert_eq!(log.dropped, 0);
    log.messages
}

fn error(text: &str) -> ConsoleMessage {
    ConsoleMessage {
        level: ConsoleLevel::Error,
        text: text.to_owned(),
        truncated: false,
    }
}

#[test]
fn listener_exceptions_reach_window_error_handlers_and_the_console() {
    let mut agent = page();
    agent
        .evaluate(
            r#"
            globalThis.log = [];
            window.onerror = function (message, source, line, column, error) {
                log.push([this === window, message, source, line, column, error.message]);
                return error.message === 'quiet';
            };
            window.addEventListener('error', (event) => {
                log.push([event instanceof ErrorEvent, event.isTrusted, event.cancelable,
                    event.bubbles, event.target === window, event.error.message]);
            });
            const body = document.body;
            body.addEventListener('ping', () => { throw new Error('loud'); });
            body.addEventListener('ping', () => { throw new Error('quiet'); });
            body.addEventListener('ping', () => log.push('third listener ran'));
            body.dispatchEvent(new Event('ping'));
            const expected = JSON.stringify([
                [true, 'Uncaught Error: loud', '', 0, 0, 'loud'],
                [true, true, true, false, true, 'loud'],
                [true, 'Uncaught Error: quiet', '', 0, 0, 'quiet'],
                [true, true, true, false, true, 'quiet'],
                'third listener ran',
            ]);
            if (JSON.stringify(log) !== expected) throw JSON.stringify(log);
        "#,
        )
        .unwrap();
    // The handler canceled only the second report.
    assert_eq!(messages(&agent), [error("Uncaught Error: loud")]);
    assert_eq!(page::take_listener_error_count(&agent), 2);
}

#[test]
fn an_error_while_reporting_is_only_logged() {
    let mut agent = page();
    agent
        .evaluate(
            r#"
            globalThis.count = 0;
            window.onerror = () => { count++; throw new TypeError('in handler'); };
            document.body.addEventListener('ping', () => { throw 'primitive'; });
            document.body.dispatchEvent(new Event('ping'));
            if (count !== 1) throw count;
        "#,
        )
        .unwrap();
    assert_eq!(
        messages(&agent),
        [
            error("Uncaught TypeError: in handler"),
            error("Uncaught primitive"),
        ]
    );
}

#[test]
fn describing_values_never_runs_page_code() {
    let mut agent = page();
    agent
        .evaluate(
            r#"
            globalThis.touched = false;
            const sneaky = { get message() { touched = true; return 'no'; },
                             toString() { touched = true; return 'no'; } };
            const proxy = new Proxy({}, { get() { touched = true; }, getOwnPropertyDescriptor() { touched = true; } });
            console.log('text', 42, null, undefined, true, sneaky, proxy, Symbol('s'), 10n);
            class Custom extends Error {}
            Custom.prototype.name = 'Custom';
            console.warn(new Custom('detail'), new RangeError(''));
            console.info({ name: 'Named', message: 'fields' });
            console.debug();
            console.assert(true, 'not shown');
            console.assert(0, 'shown', 1);
            console.assert(false);
            if (touched) throw 'page code ran';
            if (Object.prototype.toString.call(console) !== '[object console]') throw 'tag';
        "#,
        )
        .unwrap();
    let logged = messages(&agent);
    let texts: Vec<_> = logged
        .iter()
        .map(|message| (message.level, message.text.as_str()))
        .collect();
    assert_eq!(
        texts,
        [
            (
                ConsoleLevel::Log,
                "text 42 null undefined true [object Object] [object Object] Symbol(s) 10n"
            ),
            (ConsoleLevel::Warn, "Custom: detail RangeError"),
            (ConsoleLevel::Info, "Named: fields"),
            (ConsoleLevel::Debug, ""),
            (ConsoleLevel::Error, "Assertion failed: shown 1"),
            (ConsoleLevel::Error, "Assertion failed"),
        ]
    );
}

#[test]
fn the_console_buffer_is_bounded_and_marks_cut_messages() {
    let mut agent = page();
    agent
        .evaluate(&format!(
            "console.log('x'.repeat({}) + 'é'.repeat(10));
             for (let i = 0; i < {}; i++) console.log(i);",
            page::CONSOLE_MESSAGE_BYTES - 1,
            page::CONSOLE_CAPACITY + 4,
        ))
        .unwrap();
    let log = page::take_console_messages(&agent);
    assert_eq!(log.messages.len(), page::CONSOLE_CAPACITY);
    assert_eq!(log.dropped, 5);
    let first = &log.messages[0];
    assert!(first.truncated);
    // The cut falls on a character boundary below the byte limit.
    assert_eq!(first.text.len(), page::CONSOLE_MESSAGE_BYTES - 1);
    assert!(!log.messages[1].truncated);
    // Taking the log empties it.
    assert_eq!(page::take_console_messages(&agent), Default::default());
}

#[test]
fn unhandled_rejections_fire_after_the_checkpoint_and_handled_ones_follow() {
    let mut agent = page();
    agent
        .evaluate(
            r#"
            globalThis.log = [];
            window.onunhandledrejection = (event) => {
                log.push(['handler', event instanceof PromiseRejectionEvent, event.isTrusted,
                    event.cancelable, event.reason]);
                if (event.reason === 'silenced') event.preventDefault();
            };
            window.addEventListener('rejectionhandled', (event) => {
                log.push(['handled', event.cancelable, event.promise === globalThis.late, event.reason]);
            });
            globalThis.late = Promise.reject('later handled');
            Promise.reject('silenced');
            // Handled before the checkpoint ends: never reported.
            Promise.reject('caught in time').catch(() => {});
            log.push('sync');
        "#,
        )
        .unwrap();
    checkpoint(&mut agent);
    agent
        .evaluate(
            r#"
            const expected = JSON.stringify([
                'sync',
                ['handler', true, true, true, 'later handled'],
                ['handler', true, true, true, 'silenced'],
            ]);
            if (JSON.stringify(log) !== expected) throw JSON.stringify(log);
            log.length = 0;
            late.catch(() => {});
        "#,
        )
        .unwrap();
    checkpoint(&mut agent);
    agent
        .evaluate(
            r#"
            const handled = JSON.stringify([['handled', false, true, 'later handled']]);
            if (JSON.stringify(log) !== handled) throw JSON.stringify(log);
        "#,
        )
        .unwrap();
    assert_eq!(
        messages(&agent),
        [error("Uncaught (in promise) later handled")]
    );
}

#[test]
fn error_and_rejection_event_constructors_follow_their_dictionaries() {
    let mut agent = page();
    agent
        .evaluate(
            r#"
            const order = [];
            const init = {};
            for (const name of ['message', 'lineno', 'filename', 'error', 'colno', 'cancelable', 'bubbles']) {
                Object.defineProperty(init, name, { get() { order.push(name);
                    return name === 'lineno' ? 4294967297 : name === 'colno' ? -1
                        : name === 'error' ? null : name === 'bubbles' ? true : 'v'; } });
            }
            const event = new ErrorEvent('error', init);
            if (order.join() !== 'bubbles,cancelable,colno,error,filename,lineno,message') throw order.join();
            if (event.lineno !== 1 || event.colno !== 4294967295 || event.error !== null ||
                event.message !== 'v' || event.filename !== 'v' || !event.bubbles ||
                event.isTrusted || !(event instanceof Event)) throw 'fields';
            const plain = new ErrorEvent('x');
            if (plain.message !== '' || plain.lineno !== 0 || plain.error !== undefined) throw 'defaults';
            try { new PromiseRejectionEvent('x', {}); throw 'promise required'; }
            catch (error) { if (!(error instanceof TypeError)) throw error; }
            try { new PromiseRejectionEvent('x'); throw 'init required'; }
            catch (error) { if (!(error instanceof TypeError)) throw error; }
            const promise = Promise.resolve();
            const rejection = new PromiseRejectionEvent('unhandledrejection', { promise, reason: 7 });
            if (rejection.promise !== promise || rejection.reason !== 7) throw 'rejection';
            const getter = Object.getOwnPropertyDescriptor(ErrorEvent.prototype, 'message').get;
            try { getter.call(new Event('x')); throw 'brand'; }
            catch (error) { if (!(error instanceof TypeError)) throw error; }
        "#,
        )
        .unwrap();
}

#[test]
fn handler_attributes_keep_registration_order_and_return_value_rules() {
    let mut agent = page();
    agent
        .evaluate(
            r#"
            const log = [];
            const body = document.body;
            body.addEventListener('click', () => log.push('first listener'));
            const handler = function (event) { log.push(this === body ? 'handler' : 'bad this'); return false; };
            body.onclick = handler;
            body.addEventListener('click', () => log.push('last listener'));
            if (body.onclick !== handler) throw 'getter';
            const event = new MouseEvent('click', { cancelable: true });
            body.dispatchEvent(event);
            if (!event.defaultPrevented) throw 'return false cancels';
            // Replacing the value keeps the original position.
            body.onclick = () => log.push('replacement');
            body.dispatchEvent(new MouseEvent('click'));
            // Clearing removes the record; setting again appends a new one.
            body.onclick = null;
            body.onclick = () => log.push('re-added');
            body.dispatchEvent(new MouseEvent('click'));
            body.onclick = 5;
            if (body.onclick !== null) throw 'non-object';
            body.onclick = {};
            body.dispatchEvent(new MouseEvent('click'));
            const expected = [
                'first listener', 'handler', 'last listener',
                'first listener', 'replacement', 'last listener',
                'first listener', 'last listener', 're-added',
                'first listener', 'last listener',
            ];
            if (log.join() !== expected.join()) throw log.join();
            if (document.onclick !== null || window.onload !== null) throw 'defaults';
            if ('onunhandledrejection' in document.body || !('onunhandledrejection' in window)) throw 'window only';
            const desc = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'onclick');
            if (!desc.get || !desc.set || !desc.enumerable || !desc.configurable) throw 'shape';
        "#,
        )
        .unwrap();
}
