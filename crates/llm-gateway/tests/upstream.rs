//! What another process on the host can do to a fake upstream of these tests. A loopback port is
//! open to every process on the machine, and cargo-nextest runs each test in a process of its
//! own, many at once: a gateway of another test whose target held the same port before, or a
//! test that probes a port it chose, may connect to an upstream that is not its own.

use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    thread,
    time::{Duration, Instant},
};

const WAIT: Duration = Duration::from_secs(10);

/// A request to `authority` from no call of the test that owns it, as a gateway of another test
/// process sends one, read to its end: every byte it was answered.
pub fn foreign_request(authority: &str) -> Vec<u8> {
    let mut connection = TcpStream::connect(authority).unwrap();
    connection.set_read_timeout(Some(WAIT)).unwrap();
    connection
        .write_all(
            format!("POST /v1/chat/completions HTTP/1.1\r\nhost: {authority}\r\ncontent-type: application/json\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{{}}")
                .as_bytes(),
        )
        .unwrap();
    let mut answer = Vec::new();
    drop(connection.read_to_end(&mut answer));
    answer
}

/// The port of `authority`, bound by this process as another one would bind it once it is free,
/// tried for `within`; `None` while something else still holds it.
pub fn take_port(authority: &str, within: Duration) -> Option<TcpListener> {
    let deadline = Instant::now() + within;
    loop {
        if let Ok(listener) = TcpListener::bind(authority) {
            return Some(listener);
        }
        if Instant::now() > deadline {
            return None;
        }
        thread::sleep(Duration::from_millis(5));
    }
}
