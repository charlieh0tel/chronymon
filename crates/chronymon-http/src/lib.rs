//! The little HTTP server the web view uses.
//!
//! Not a web framework and not trying to be: it accepts a connection,
//! hands the caller a path, and writes back a status and a body.  That
//! is the whole of what the web view needs, and an async runtime and a
//! routing crate would be a large dependency for it.
//!
//! A copy of smartclock's, by the same author, kept a crate of its own
//! so the limits below stay in one place.

use std::io::BufRead as _;
use std::io::BufReader;
use std::io::Read as _;
use std::io::Write as _;
use std::net::TcpListener;
use std::net::TcpStream;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::Duration;
use std::time::Instant;

/// The longest request line served.
///
/// A request line is a method, a path and a version.  Without a cap,
/// `read_line` grows a string until the client sends a newline, which
/// a hostile one never does: on a local network that is memory filling
/// at wire speed from a single connection.
const MAX_REQUEST: u64 = 8192;

/// How long a client may take to send its request, or to read its
/// answer, before it is dropped.
///
/// Without these a connection that sends nothing holds its thread for
/// as long as the process runs, and a client that never reads holds one
/// in `write`.  Both are one line of shell to arrange.
///
/// The request's is a deadline for the whole of it, not a timeout per
/// read.  A timeout per read restarts with every byte, so a client
/// sending one byte every few seconds held a thread for as long as it
/// liked, and thirty-two of them held all of them.
const REQUEST_DEADLINE: Duration = Duration::from_secs(10);
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);

/// How many requests may be in flight at once.
///
/// Each takes a thread.  A page load and a poll are short, so this is
/// about surviving a flood rather than about throughput; past it,
/// clients are told the server is busy instead of the machine being
/// taken down.
const MAX_INFLIGHT: usize = 32;

/// Why the server could not start.
#[derive(Debug, thiserror::Error)]
pub enum ServeError {
    /// The address could not be bound: taken, not this host's, or not
    /// permitted.
    #[error("binding {listen}: {source}")]
    Bind {
        /// The address asked for.
        listen: String,
        /// What the bind said.
        #[source]
        source: std::io::Error,
    },
}

/// What the caller wants sent back.
#[derive(Debug)]
pub struct Response {
    /// The status line, such as `200 OK`.
    pub status: &'static str,
    /// The media type, including any charset.
    pub kind: &'static str,
    /// The body.  Sent as bytes; `Content-Length` counts bytes, not
    /// characters, which is why this is a String rather than a &str.
    pub body: String,
}

impl Response {
    /// A successful reply.
    pub fn ok(kind: &'static str, body: String) -> Self {
        Self {
            status: "200 OK",
            kind,
            body,
        }
    }

    /// Nothing is served at that path.
    pub fn not_found() -> Self {
        Self {
            status: "404 Not Found",
            kind: "text/plain; charset=utf-8",
            body: "not found\n".to_owned(),
        }
    }
}

/// The `key=value` pairs of a query string, in order, each value
/// percent-decoded.  A pair with no `=` is skipped.
pub fn pairs(query: &str) -> impl Iterator<Item = (&str, String)> {
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .map(|(key, value)| (key, decode(value)))
}

/// The decoded value of the first `key=` in a query string.
pub fn value(query: &str, key: &str) -> Option<String> {
    pairs(query)
        .find(|(k, _)| *k == key)
        .map(|(_, value)| value)
}

/// Percent-decode one query-string value.
///
/// `URLSearchParams` writes a list's separator as `%2C`, so without
/// decoding the server is handed one column called
/// `frequency%2Cskew` and says, correctly, that it does not serve it.
///
/// Bytes rather than chars, because a percent escape encodes a byte and
/// a multi-byte character arrives as several of them.  Anything that
/// is not a well-formed escape is kept as written: a stray `%` in a
/// value is not worth refusing a request over.
fn decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                // A literal plus is `%2B`; a bare one is a space in
                // form encoding, and a space is not a column name.
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
                match hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    Some(byte) => {
                        out.push(byte);
                        i += 3;
                    }
                    None => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// One connected client, counted for as long as this is held.
///
/// A guard rather than a decrement at the end of the thread, so a
/// panic while answering cannot leak a slot.
struct Slot(Arc<AtomicUsize>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Serve `answer` on `listen` until the process ends.
///
/// `answer` is given the request path, including any query string, and
/// returns what to send.  It runs on its own thread per request, so it
/// may block; the timeouts above bound how long a client can make it
/// wait, not how long it may take.
pub fn serve<F>(listen: &str, answer: F) -> Result<(), ServeError>
where
    F: Fn(&str) -> Response + Send + Sync + 'static,
{
    let listener = TcpListener::bind(listen).map_err(|source| ServeError::Bind {
        listen: listen.to_owned(),
        source,
    })?;
    let answer = Arc::new(answer);
    let inflight = Arc::new(AtomicUsize::new(0));

    for stream in listener.incoming() {
        let stream = match stream {
            Ok(stream) => stream,
            // One client failing to connect is not a reason to stop
            // serving the others.
            Err(e) => {
                eprintln!("rejected a connection: {e}");
                continue;
            }
        };
        if inflight.load(Ordering::Relaxed) >= MAX_INFLIGHT {
            // Answered rather than dropped: a bare reset reads as the
            // server having died.
            let _ = write_response(
                &stream,
                &Response {
                    status: "503 Service Unavailable",
                    kind: "text/plain; charset=utf-8",
                    body: "busy\n".to_owned(),
                },
            );
            continue;
        }
        inflight.fetch_add(1, Ordering::Relaxed);
        let slot = Slot(Arc::clone(&inflight));
        let answer = Arc::clone(&answer);
        let spawned = thread::Builder::new()
            .name("chronymon-http".to_owned())
            .spawn(move || {
                let _slot = slot;
                handle(&stream, answer.as_ref());
            });
        if let Err(e) = spawned {
            eprintln!("could not serve a request: {e}");
        }
    }
    Ok(())
}

/// Read one request and write one answer.
fn handle<F>(stream: &TcpStream, answer: &F)
where
    F: Fn(&str) -> Response,
{
    handle_within(stream, answer, REQUEST_DEADLINE);
}

/// As [`handle`], with the whole request due within `deadline`.
fn handle_within<F>(stream: &TcpStream, answer: &F, deadline: Duration)
where
    F: Fn(&str) -> Response,
{
    let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));

    let mut reader = BufReader::new(Deadline {
        stream,
        until: Instant::now() + deadline,
    });
    let mut line = String::new();
    // Capped, so a line that never ends costs 8 KiB rather than the
    // machine.
    let read = match (&mut reader).take(MAX_REQUEST + 1).read_line(&mut line) {
        Ok(0) | Err(_) => return,
        Ok(read) => read,
    };
    if read as u64 > MAX_REQUEST {
        let _ = write_response(
            stream,
            &Response {
                status: "431 Request Header Fields Too Large",
                kind: "text/plain; charset=utf-8",
                body: "request line too long\n".to_owned(),
            },
        );
        return;
    }

    // "GET /api/status HTTP/1.1".  Only the path is of interest; the
    // server reads no body and needs no header.
    let path = line.split_whitespace().nth(1).unwrap_or("/");
    let _ = write_response(stream, &answer(path));
}

/// A stream that stops reading at a fixed moment, however the bytes
/// trickle in.
struct Deadline<'a> {
    stream: &'a TcpStream,
    until: Instant,
}

impl std::io::Read for Deadline<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let left = self.until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(std::io::ErrorKind::TimedOut.into());
        }
        self.stream.set_read_timeout(Some(left))?;
        let mut stream = self.stream;
        stream.read(buf)
    }
}

fn write_response(mut stream: &TcpStream, response: &Response) -> std::io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {}\r\n\
         Content-Type: {}\r\n\
         Content-Length: {}\r\n\
         Cache-Control: no-store\r\n\
         Connection: close\r\n\r\n{}",
        response.status,
        response.kind,
        response.body.len(),
        response.body
    )
}

#[cfg(test)]
mod tests {
    use super::decode;
    use super::pairs;
    use super::value;

    /// What a browser actually sends for a list: `URLSearchParams`
    /// encodes the separator.
    #[test]
    fn a_percent_encoded_list_decodes() {
        assert_eq!(decode("frequency%2Cskew"), "frequency,skew");
    }

    #[test]
    fn a_plus_is_a_space_and_a_percent_2b_is_a_plus() {
        assert_eq!(decode("one+two"), "one two");
        assert_eq!(decode("one%2Btwo"), "one+two");
    }

    /// A malformed escape is kept rather than refused.  A stray percent
    /// in a value is not worth failing a request over, and the column
    /// check downstream rejects anything that is not a real column
    /// anyway.
    #[test]
    fn a_broken_escape_survives() {
        assert_eq!(decode("100%"), "100%");
        assert_eq!(decode("%zz"), "%zz");
        assert_eq!(decode("%2"), "%2");
    }

    #[test]
    fn multibyte_characters_survive_the_round_trip() {
        assert_eq!(decode("%C2%B5s"), "\u{b5}s");
    }

    #[test]
    fn pairs_are_split_and_decoded_and_the_first_value_wins() {
        let query = "source=192.0.2.1&columns=a%2Cb&bare&source=other";
        assert_eq!(value(query, "source").as_deref(), Some("192.0.2.1"));
        assert_eq!(value(query, "columns").as_deref(), Some("a,b"));
        assert_eq!(value(query, "missing"), None);
        assert_eq!(pairs(query).count(), 3);
    }

    use super::Response;
    use super::handle_within;
    use std::io::Write as _;
    use std::net::TcpListener;
    use std::net::TcpStream;
    use std::time::Duration;
    use std::time::Instant;

    #[test]
    fn a_request_that_trickles_in_is_cut_off_at_the_deadline() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("an address");
        // A byte every 200 ms for four seconds: each read succeeds
        // well inside any per-read timeout.
        let client = std::thread::spawn(move || {
            let mut stream = TcpStream::connect(address).expect("connect");
            for _ in 0..20 {
                if stream.write_all(b"G").is_err() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        });
        let (server, _) = listener.accept().expect("accept");
        let started = Instant::now();
        handle_within(
            &server,
            &|_: &str| Response::not_found(),
            Duration::from_secs(1),
        );
        let took = started.elapsed();
        drop(server);
        let _ = client.join();
        assert!(took < Duration::from_secs(2), "held for {took:?}");
    }
}
