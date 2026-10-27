//! A client of chronyd's control protocol, synchronous, over its Unix
//! socket or its UDP command port.
//!
//! Over the Unix socket chronyd replies to the client's own socket, so the client binds one
//! where chronyd can reach it: in a private directory beside chronyd's
//! socket, as chronyc does (`open_unix_socket` in `client.c`).  The
//! directory is searchable but not listable and the socket writable by
//! all, so chronyd's user can reply; it goes when the client does.
//!
//! Over UDP chronyd answers only the commands open to the network
//! (`opencommands`, 4.7 on) from hosts `cmdallow` allows, localhost by
//! default; any other command it refuses, `Status::Unauthorized`.

use std::fmt;
use std::fs;
use std::fs::DirBuilder;
use std::fs::Permissions;
use std::io;
use std::io::ErrorKind;
use std::io::Read as _;
use std::net::IpAddr;
use std::net::Ipv4Addr;
use std::net::Ipv6Addr;
use std::net::SocketAddr;
use std::net::UdpSocket;
use std::os::unix::fs::DirBuilderExt as _;
use std::os::unix::fs::PermissionsExt as _;
use std::os::unix::net::UnixDatagram;
use std::path::Path;
use std::path::PathBuf;
use std::process;
use std::str::FromStr;
use std::time::Duration;
use std::time::Instant;

use crate::error::Error;
use crate::error::ParseServerError;
use crate::protocol::Decode;
use crate::protocol::REPLY_HEADER_LENGTH;
use crate::protocol::ReplyHeader;
use crate::protocol::Request;
use crate::protocol::SourceCount;
use crate::protocol::SourceName;
use crate::report::Activity;
use crate::report::NtpData;
use crate::report::SelectData;
use crate::report::SourceData;
use crate::report::SourceStats;
use crate::report::Tracking;

/// Where Debian's and Ubuntu's chronyd listen.
pub const DEFAULT_SOCKET: &str = "/run/chrony/chronyd.sock";

/// chronyd's UDP command port, `cmdport`'s default.
pub const DEFAULT_PORT: u16 = 323;

/// Where chronyd takes control requests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Server {
    /// Its Unix socket.
    Unix(PathBuf),
    /// Its UDP command port.
    Udp(SocketAddr),
}

impl FromStr for Server {
    type Err = ParseServerError;

    /// A path, starting with `/`, or an address, with or without a
    /// port, as chronyc's `-h` takes them: `/run/chrony/chronyd.sock`,
    /// `127.0.0.1`, `[::1]:323`.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        if text.starts_with('/') {
            return Ok(Self::Unix(PathBuf::from(text)));
        }
        text.parse::<SocketAddr>()
            .or_else(|_| {
                text.parse::<IpAddr>()
                    .map(|ip| SocketAddr::new(ip, DEFAULT_PORT))
            })
            .map(Self::Udp)
            .map_err(|_| ParseServerError(text.to_owned()))
    }
}

impl fmt::Display for Server {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unix(path) => write!(f, "{}", path.display()),
            Self::Udp(address) => write!(f, "udp {address}"),
        }
    }
}

/// How long one attempt waits for its reply, and how many are made,
/// before the request is given up.  chronyc's defaults are a 1 s
/// timeout and 3 attempts, doubling the timeout each time; a local
/// socket either answers at once or not at all.
const TIMEOUT: Duration = Duration::from_secs(1);
const ATTEMPTS: u16 = 3;

/// The largest reply chrony defines, with room to spare.
const RECEIVE_BUFFER: usize = 1024;
/// Random characters in the private directory's name.
const RANDOM_NAME_LENGTH: usize = 16;

const DIRECTORY_MODE: u32 = 0o711;
const SOCKET_MODE: u32 = 0o666;

/// A connection to chronyd.
#[derive(Debug)]
pub struct Client {
    transport: Transport,
    sequence: u32,
}

impl Client {
    /// Connect to chronyd at `server`.
    pub fn connect(server: &Server) -> Result<Self, Error> {
        let transport = match server {
            Server::Unix(path) => Transport::Unix(UnixSocket::connect(path)?),
            Server::Udp(address) => Transport::Udp(connect_udp(*address)?),
        };
        Ok(Self {
            transport,
            // Any start will do; chronyc's is random too.
            sequence: random_sequence(),
        })
    }

    /// `tracking`.
    pub fn tracking(&mut self) -> Result<Tracking, Error> {
        self.query(Request::Tracking)
    }

    /// How many sources chronyd has; they are asked for by index below
    /// that.
    pub fn source_count(&mut self) -> Result<u32, Error> {
        self.query::<SourceCount>(Request::SourceCount)
            .map(|count| count.0)
    }

    /// `sources`, for the source at `index`.
    pub fn source_data(&mut self, index: u32) -> Result<SourceData, Error> {
        self.query(Request::SourceData(index))
    }

    /// `sourcestats`, for the source at `index`.
    pub fn source_stats(&mut self, index: u32) -> Result<SourceStats, Error> {
        self.query(Request::SourceStats(index))
    }

    /// `selectdata`, for the source at `index`.
    pub fn select_data(&mut self, index: u32) -> Result<SelectData, Error> {
        self.query(Request::SelectData(index))
    }

    /// `ntpdata`, for the NTP source at `address`.
    pub fn ntp_data(&mut self, address: IpAddr) -> Result<NtpData, Error> {
        self.query(Request::NtpData(address))
    }

    /// `sourcename`: the name the source at `address` was configured
    /// with.
    pub fn source_name(&mut self, address: IpAddr) -> Result<String, Error> {
        self.query::<SourceName>(Request::SourceName(address))
            .map(|name| name.0)
    }

    /// `activity`.
    pub fn activity(&mut self) -> Result<Activity, Error> {
        self.query(Request::Activity)
    }

    /// Send `request` and wait for its reply, resending on timeout.
    /// Datagrams that are not the reply -- another sequence, a late
    /// answer to an earlier attempt -- are ignored, as chronyc does.
    fn query<T: Decode>(&mut self, request: Request) -> Result<T, Error> {
        self.sequence = self.sequence.wrapping_add(1);
        let mut buffer = [0; RECEIVE_BUFFER];
        for attempt in 0..ATTEMPTS {
            self.transport
                .send(&request.encode(self.sequence, attempt))?;
            let deadline = Instant::now() + TIMEOUT;
            loop {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    break;
                }
                self.transport.set_read_timeout(Some(remaining))?;
                let length = match self.transport.recv(&mut buffer) {
                    Ok(length) => length,
                    Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                        break;
                    }
                    Err(e) => return Err(e.into()),
                };
                let reply = &buffer[..length];
                let Some(header) = ReplyHeader::parse(reply) else {
                    continue;
                };
                if header.command != request.command() || header.sequence != self.sequence {
                    continue;
                }
                header.status().map_err(Error::Status)?;
                return Ok(T::decode(
                    header.command,
                    header.reply,
                    &reply[REPLY_HEADER_LENGTH..],
                )?);
            }
        }
        Err(Error::Timeout {
            attempts: ATTEMPTS,
            timeout: TIMEOUT,
        })
    }
}

/// The client's socket, connected to chronyd's.
#[derive(Debug)]
enum Transport {
    Unix(UnixSocket),
    Udp(UdpSocket),
}

impl Transport {
    fn send(&self, bytes: &[u8]) -> io::Result<usize> {
        match self {
            Self::Unix(unix) => unix.socket.send(bytes),
            Self::Udp(socket) => socket.send(bytes),
        }
    }

    fn recv(&self, buffer: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Unix(unix) => unix.socket.recv(buffer),
            Self::Udp(socket) => socket.recv(buffer),
        }
    }

    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        match self {
            Self::Unix(unix) => unix.socket.set_read_timeout(timeout),
            Self::Udp(socket) => socket.set_read_timeout(timeout),
        }
    }
}

/// A UDP socket connected to chronyd's command port, so only its
/// datagrams are received and a port with no chronyd behind it is
/// refused at once, as the Unix socket is.
fn connect_udp(address: SocketAddr) -> Result<UdpSocket, Error> {
    let any = match address {
        SocketAddr::V4(_) => IpAddr::V4(Ipv4Addr::UNSPECIFIED),
        SocketAddr::V6(_) => IpAddr::V6(Ipv6Addr::UNSPECIFIED),
    };
    UdpSocket::bind(SocketAddr::new(any, 0))
        .and_then(|socket| {
            socket.connect(address)?;
            socket.set_read_timeout(Some(TIMEOUT))?;
            Ok(socket)
        })
        .map_err(|source| Error::Connect {
            server: Server::Udp(address),
            source,
        })
}

/// The client's own Unix socket, connected to chronyd's.
#[derive(Debug)]
struct UnixSocket {
    socket: UnixDatagram,
    /// The private directory holding the socket, removed on drop.
    directory: PathBuf,
}

impl UnixSocket {
    fn connect(server: &Path) -> Result<Self, Error> {
        // chronyd's socket first, as chronyc does: when chronyd is not
        // running or this user may not use it, that is what to say,
        // and there is nothing of the client's own to clean up.
        UnixDatagram::unbound()
            .and_then(|probe| probe.connect(server))
            .map_err(|source| Error::Connect {
                server: Server::Unix(server.to_path_buf()),
                source,
            })?;
        let parent = server.parent().unwrap_or(Path::new("/"));
        let directory = parent.join(format!(
            "chronymon.{}.{}",
            process::id(),
            random_name().map_err(|source| Error::Socket {
                path: parent.to_path_buf(),
                source,
            })?
        ));
        DirBuilder::new()
            .mode(DIRECTORY_MODE)
            .create(&directory)
            .map_err(|source| Error::Connect {
                server: Server::Unix(server.to_path_buf()),
                source,
            })?;
        let path = directory.join("sock");
        let bound = UnixDatagram::bind(&path)
            .and_then(|socket| {
                fs::set_permissions(&path, Permissions::from_mode(SOCKET_MODE))?;
                fs::set_permissions(&directory, Permissions::from_mode(DIRECTORY_MODE))?;
                socket.set_read_timeout(Some(TIMEOUT))?;
                Ok(socket)
            })
            .map_err(|source| Error::Socket {
                path: path.clone(),
                source,
            });
        let socket = match bound {
            Ok(socket) => socket,
            Err(e) => {
                remove(&directory);
                return Err(e);
            }
        };
        let unix = Self { socket, directory };
        unix.socket
            .connect(server)
            .map_err(|source| Error::Connect {
                server: Server::Unix(server.to_path_buf()),
                source,
            })?;
        Ok(unix)
    }
}

impl Drop for UnixSocket {
    fn drop(&mut self) {
        remove(&self.directory);
    }
}

fn remove(directory: &Path) {
    // Best effort: a stale directory is harmless, and nothing could be
    // done about a failure here anyway.
    let _ = fs::remove_dir_all(directory);
}

fn random_bytes<const N: usize>() -> io::Result<[u8; N]> {
    let mut bytes = [0; N];
    fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes)
}

/// A name no one can guess, so the directory cannot be made in advance
/// by someone else.
fn random_name() -> io::Result<String> {
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    Ok(random_bytes::<RANDOM_NAME_LENGTH>()?
        .into_iter()
        .map(|byte| char::from(ALPHABET[usize::from(byte) % ALPHABET.len()]))
        .collect())
}

fn random_sequence() -> u32 {
    random_bytes().map(u32::from_ne_bytes).unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use std::thread;

    use super::*;
    use crate::error::Status;

    /// Where a request's command and sequence are, and a reply's.
    const REQUEST_COMMAND: std::ops::Range<usize> = 4..6;
    const REQUEST_SEQUENCE: std::ops::Range<usize> = 8..12;
    const REPLY_COMMAND: std::ops::Range<usize> = 4..6;
    const REPLY_STATUS: std::ops::Range<usize> = 8..10;
    const REPLY_SEQUENCE: std::ops::Range<usize> = 16..20;
    /// `PROTO_VERSION_NUMBER` and `PKT_TYPE_CMD_REPLY`; `STT_UNAUTH`.
    const REPLY_START: [u8; 2] = [6, 2];
    const UNAUTHORIZED: u16 = 2;

    #[test]
    fn parses_where_chronyd_is() {
        let parse = |text: &str| text.parse::<Server>();
        assert_eq!(
            parse("/run/chrony/chronyd.sock"),
            Ok(Server::Unix(PathBuf::from("/run/chrony/chronyd.sock")))
        );
        assert_eq!(
            parse("127.0.0.1"),
            Ok(Server::Udp(SocketAddr::from((
                [127, 0, 0, 1],
                DEFAULT_PORT
            ))))
        );
        assert_eq!(
            parse("::1"),
            Ok(Server::Udp(SocketAddr::new(
                IpAddr::V6(Ipv6Addr::LOCALHOST),
                DEFAULT_PORT
            )))
        );
        assert_eq!(
            parse("[::1]:1323"),
            Ok(Server::Udp(SocketAddr::new(
                IpAddr::V6(Ipv6Addr::LOCALHOST),
                1323
            )))
        );
        assert!(parse("chronyd.sock").is_err());
        assert!(parse("localhost").is_err());
    }

    /// A stand-in chronyd on a UDP port, refusing the one request it
    /// answers as chronyd refuses a command it does not open.
    #[test]
    fn asks_over_udp() {
        let chronyd = UdpSocket::bind("127.0.0.1:0").expect("a port");
        let address = chronyd.local_addr().expect("its address");
        let answer = thread::spawn(move || {
            let mut request = [0; RECEIVE_BUFFER];
            let (length, client) = chronyd.recv_from(&mut request).expect("a request");
            let mut reply = vec![0; length];
            reply[..REPLY_START.len()].copy_from_slice(&REPLY_START);
            reply[REPLY_COMMAND].copy_from_slice(&request[REQUEST_COMMAND]);
            reply[REPLY_STATUS].copy_from_slice(&UNAUTHORIZED.to_be_bytes());
            reply[REPLY_SEQUENCE].copy_from_slice(&request[REQUEST_SEQUENCE]);
            chronyd.send_to(&reply, client).expect("a reply");
        });
        let mut client = Client::connect(&Server::Udp(address)).expect("a client");
        assert!(matches!(
            client.select_data(0),
            Err(Error::Status(Status::Unauthorized))
        ));
        answer.join().expect("answered");
    }

    #[test]
    fn a_port_without_chronyd_is_refused() {
        // Bound, so no one else has it, then closed.
        let address = UdpSocket::bind("127.0.0.1:0")
            .and_then(|socket| socket.local_addr())
            .expect("a free port");
        let mut client = Client::connect(&Server::Udp(address)).expect("a client");
        assert!(matches!(
            client.tracking(),
            Err(Error::Io(e)) if e.kind() == ErrorKind::ConnectionRefused
        ));
    }
}
