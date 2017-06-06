use net2::TcpBuilder;
use net2::TcpStreamExt;
use std::fmt::Arguments;
use std::io::Read;
use std::io::Write;
use std::io;
use std::net::TcpStream;
use std::os::unix::io::{AsRawFd, RawFd};
use std::str;

use openssl::ssl;
use openssl::error::ErrorStack;
use openssl::ssl::{
    HandshakeError,
    MidHandshakeSslStream,
    SslConnectorBuilder,
    SslMethod,
    SslStream,
    SslContextBuilder,
};

use logger::Logger;
use logger::LogFile;
use wire::{Cmd, Msg};
use wire;

pub struct Conn {
    nick: String,
    hostname: String,
    realname: String,

    /// servername to be used in PING messages. Read from 002 RPL_YOURHOST. `None` until 002.
    host: Option<String>,

    serv_addr: String,

    /// The TCP connection to the server.
    stream: ConnStream,

    status: ConnStatus,

    serv_name: String,

    /// _Partial_ messages collected here until they make a complete message.
    buf: Vec<u8>,
}

enum ConnStream {
    Tcp(TcpStream),
    TcpSsl(MidHandshakeSslStream<TcpStream>),
    Ssl(SslStream<TcpStream>),
}

impl ConnStream {
    fn write(&mut self) -> &mut Write {
        match self {
            &mut ConnStream::Tcp(ref mut  s) => s,
            &mut ConnStream::TcpSsl(ref mut s) => panic!("write(): In the middle of ssl handshake"),
            &mut ConnStream::Ssl(ref mut s) => s,
        }
    }

    fn read(&mut self) -> &mut Read {
        match self {
            &mut ConnStream::Tcp(ref mut  s) => s,
            &mut ConnStream::TcpSsl(ref mut s) => panic!("read(): In the middle of ssl handshake"),
            &mut ConnStream::Ssl(ref mut s) => s,
        }
    }
}

pub enum SslConnectStatus {
    WantWrite,
    WantRead,
    JustConnected,
    AlreadyConnected,
}

/// How many ticks to wait before sending a ping to the server.
const PING_TICKS: u8 = 60;
/// How many ticks to wait after sending a ping to the server to consider a disconnect.
const PONG_TICKS: u8 = 60;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum ConnStatus {
    /// Need to introduce self
    Introduce,
    PingPong {
        /// Ticks passed since last time we've heard from the server.
        /// Reset on each message. After `PING_TICKS` ticks we send a PING message and move to
        /// `WaitPong` state.
        ticks_passed: u8,
    },
    WaitPong {
        /// Ticks passed since we sent a PING to the server.
        /// After a message move to `PingPong` state. On timeout we reset the connection.
        ticks_passed: u8,
    },
}

#[derive(Debug)]
pub enum ConnEv {
    Disconnected,
    Err(io::Error),
    Msg(Msg),
}

fn init_stream(serv_addr: &str) -> TcpStream {
    let stream = TcpBuilder::new_v4().unwrap().to_tcp_stream().unwrap();
    stream.set_nonblocking(true).unwrap();
    // This will fail with EINPROGRESS. Socket will be ready for writing when the connection is
    // established (check POLLOUT).
    let _ = stream.connect(serv_addr);
    stream
}

impl Conn {
    pub fn new(serv_addr: &str, serv_name: &str, nick: &str, hostname: &str, realname: &str) -> Conn {
        Conn {
            nick: nick.to_owned(),
            hostname: hostname.to_owned(),
            realname: realname.to_owned(),
            host: None,
            serv_addr: serv_addr.to_owned(),
            stream: ConnStream::Tcp(init_stream(serv_addr)),
            status: ConnStatus::Introduce,
            serv_name: serv_name.to_owned(),
            buf: vec![],
        }
    }

    pub fn write(&mut self) -> &mut Write {
        self.stream.write()
    }

    pub fn read(&mut self) -> &mut Read {
        self.stream.read()
    }

    pub fn ssl_connect(&mut self) -> SslConnectStatus {
        let (new_stream, ret) = {
            match self.stream {
                ConnStream::Tcp(ref mut tcp_stream) => {
                    let mut builder: SslConnectorBuilder = SslConnectorBuilder::new(SslMethod::tls()).unwrap();
                    {
                        let ctx: &mut SslContextBuilder = builder.builder_mut();
                        ctx.set_verify(ssl::SSL_VERIFY_NONE);
                    }
                    let connector = builder.build();

                    let old_stream = 
                        ::std::mem::replace(
                            tcp_stream,
                            unsafe { ::std::mem::uninitialized() });

                    match connector.danger_connect_without_providing_domain_for_certificate_verification_and_server_name_indication(old_stream) {
                        Err(HandshakeError::SetupFailure(_)) => {
                            panic!("ssl connect failed: setup failure");
                        }
                        Err(HandshakeError::Failure(mid)) => {
                            panic!("ssl connect failed: failure ({:?})", mid.error());
                        }
                        Err(HandshakeError::Interrupted(mid)) => {
                            let status = 
                                match mid.error() {
                                    &ssl::Error::WantRead(_) =>
                                        SslConnectStatus::WantRead,
                                    &ssl::Error::WantWrite(_) =>
                                        SslConnectStatus::WantWrite,
                                    ref other =>
                                        panic!("ssl connect failed: {:?}", other),
                                };

                            (ConnStream::TcpSsl(mid), status)
                        }
                        Ok(ssl_stream) =>
                            (ConnStream::Ssl(ssl_stream), SslConnectStatus::JustConnected),
                    }
                },
                ConnStream::TcpSsl(ref mut mid) => {
                    let old_mid = 
                        ::std::mem::replace(
                            mid,
                            unsafe { ::std::mem::uninitialized() });

                    match old_mid.handshake() {
                        Err(HandshakeError::SetupFailure(_)) => {
                            panic!("ssl connect failed: setup failure");
                        }
                        Err(HandshakeError::Failure(mid)) => {
                            panic!("ssl connect failed: failure ({:?})", mid.error());
                        }
                        Err(HandshakeError::Interrupted(mid)) => {
                            let status = 
                                match mid.error() {
                                    &ssl::Error::WantRead(_) =>
                                        SslConnectStatus::WantRead,
                                    &ssl::Error::WantWrite(_) =>
                                        SslConnectStatus::WantWrite,
                                    ref other =>
                                        panic!("ssl connect failed: {:?}", other),
                                };

                            (ConnStream::TcpSsl(mid), status)
                        }
                        Ok(ssl_stream) =>
                            (ConnStream::Ssl(ssl_stream), SslConnectStatus::JustConnected),
                    }
                },
                ConnStream::Ssl(ref mut ssl_stream) => {
                    let ssl_stream = 
                        ::std::mem::replace(
                            ssl_stream,
                            unsafe { ::std::mem::uninitialized() });

                    (ConnStream::Ssl(ssl_stream), SslConnectStatus::AlreadyConnected)
                },
            }
        };

        let uninitialized = ::std::mem::replace(&mut self.stream, new_stream);
        unsafe { ::std::mem::forget(uninitialized); }
        ret
    }

    pub fn reconnect(&mut self) {
        self.stream = ConnStream::Tcp(init_stream(&self.serv_addr));
        self.status = ConnStatus::Introduce;
    }

    /// Get the RawFd, to be used with select() or other I/O multiplexer.
    pub fn get_raw_fd(&self) -> RawFd {
        match self.stream {
            ConnStream::Tcp(ref s) => s.as_raw_fd(),
            ConnStream::TcpSsl(ref s) => s.get_ref().as_raw_fd(),
            ConnStream::Ssl(ref s) => s.get_ref().as_raw_fd(),
        }
    }

    pub fn get_serv_name(&self) -> &str {
        &self.serv_name
    }
}

impl Conn {

    ////////////////////////////////////////////////////////////////////////////
    // Tick handling

    pub fn tick(&mut self, evs: &mut Vec<ConnEv>, mut debug_out: LogFile) {
        match self.status {
            ConnStatus::Introduce => {},
            ConnStatus::PingPong { ticks_passed } => {
                if ticks_passed + 1 == PING_TICKS {
                    match self.host {
                        None => {
                            debug_out.write_line(
                                format_args!("{}: Can't send PING, host unknown", self.serv_name));
                        }
                        Some(ref host_) => {
                            debug_out.write_line(
                                format_args!("{}: Ping timeout, sending PING", self.serv_name));
                            wire::ping(self.stream.write(), host_).unwrap();;
                        }
                    }
                    self.status = ConnStatus::WaitPong { ticks_passed: 0 };
                } else {
                    self.status = ConnStatus::PingPong { ticks_passed: ticks_passed + 1 };
                }
            }
            ConnStatus::WaitPong { ticks_passed } => {
                if ticks_passed + 1 == PONG_TICKS {
                    evs.push(ConnEv::Disconnected);
                    self.status = ConnStatus::Introduce;
                } else {
                    self.status = ConnStatus::WaitPong { ticks_passed: ticks_passed + 1 };
                }
            }
        }
    }

    fn reset_ticks(&mut self) {
        match self.status {
            ConnStatus::Introduce => {},
            _ => { self.status = ConnStatus::PingPong { ticks_passed: 0 }; }
        }
    }

    ////////////////////////////////////////////////////////////////////////////
    // Sending messages

    fn introduce(&mut self) {
        wire::user(&self.hostname, &self.realname, self.stream.write()).unwrap();
        wire::nick(&self.nick, self.stream.write()).unwrap();
    }

    ////////////////////////////////////////////////////////////////////////////
    // Receiving messages

    pub fn read_incoming_msg(&mut self, evs: &mut Vec<ConnEv>, logger: &mut Logger) {
        let mut read_buf: [u8; 512] = [0; 512];

        // Handle disconnects
        match self.read().read(&mut read_buf) {
            Err(err) => {
                evs.push(ConnEv::Err(err));
            }
            Ok(bytes_read) => {
                logger.get_debug_logs().write_line(
                    format_args!("{} read {:?} bytes", self.serv_name, bytes_read));
                self.reset_ticks();
                self.add_to_msg_buf(&read_buf[ 0 .. bytes_read ]);
                self.handle_msgs(evs, logger);
                if bytes_read == 0 {
                    evs.push(ConnEv::Disconnected);
                }
            }
        }
    }

    fn add_to_msg_buf(&mut self, slice: &[u8]) {
        // Some invisible ASCII characters causing glitches on some terminals,
        // we filter those out here.
        self.buf.extend(slice.iter().filter(|c| **c != 0x1 /* SOH */ ||
                                                **c != 0x2 /* STX */ ||
                                                **c != 0x0 /* NUL */ ||
                                                **c != 0x4 /* EOT */ ));
    }

    fn handle_msgs(&mut self, evs: &mut Vec<ConnEv>, logger: &mut Logger) {
        while let Some(msg) = Msg::read(&mut self.buf, Some(logger.get_raw_serv_logs(&self.serv_name))) {
            self.handle_msg(msg, evs, logger);
        }
    }

    fn handle_msg(&mut self, msg: Msg, evs: &mut Vec<ConnEv>, logger: &mut Logger) {
        if let &Msg { cmd: Cmd::PING { ref server }, .. } = &msg {
            wire::pong(server, self.write()).unwrap();
        }

        if let ConnStatus::Introduce = self.status {
            self.introduce();
            self.status = ConnStatus::PingPong { ticks_passed: 0 };
        }

        if let &Msg { cmd: Cmd::Reply { num: 002, ref params }, .. } = &msg {
            // 002    RPL_YOURHOST
            //        "Your host is <servername>, running version <ver>"

            // An example <servername>: cherryh.freenode.net[149.56.134.238/8001]

            match parse_servername(params) {
                None => {
                    logger.get_debug_logs().write_line(
                        format_args!("{} Can't parse hostname from params: {:?}",
                                     self.serv_name, params));
                }
                Some(host) => {
                    logger.get_debug_logs().write_line(
                        format_args!("{} host: {}", self.serv_name, host));
                    self.host = Some(host);
                }
            }
        }

        evs.push(ConnEv::Msg(msg));
    }
}

macro_rules! try_opt {
    ($expr:expr) => (match $expr {
        Option::Some(val) => val,
        Option::None => {
            return Option::None
        }
    })
}

/// Try to parse servername in a 002 RPL_YOURHOST reply
fn parse_servername(params: &[String]) -> Option<String> {
    let msg = try_opt!(params.get(1).or(params.get(0)));
    let slice1 = &msg[13..];
    let servername_ends = try_opt!(wire::find_byte(slice1.as_bytes(), b'['));
    Some((&slice1[..servername_ends]).to_owned())
}

// impl Read for ConnStream {
//     fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
//         match self {
//             &mut ConnStream::Tcp(ref mut s) => s.read(buf),
//             &mut ConnStream::Ssl(ref mut s) => s.read(buf),
//         }
//     }
// }
// 
// impl Write for ConnStream {
//     fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
//         match self {
//             &mut ConnStream::Tcp(ref mut s) => s.write(buf),
//             &mut ConnStream::Ssl(ref mut s) => s.write(buf),
//         }
//     }
// 
//     fn flush(&mut self) -> io::Result<()> {
//         match self {
//             &mut ConnStream::Tcp(ref mut s) => s.flush(),
//             &mut ConnStream::Ssl(ref mut s) => s.flush(),
//         }
//     }
// 
//     fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
//         match self {
//             &mut ConnStream::Tcp(ref mut s) => s.write_all(buf),
//             &mut ConnStream::Ssl(ref mut s) => s.write_all(buf),
//         }
//     }
// 
//     fn write_fmt(&mut self, fmt: Arguments) -> io::Result<()> {
//         match self {
//             &mut ConnStream::Tcp(ref mut s) => s.write_fmt(fmt),
//             &mut ConnStream::Ssl(ref mut s) => s.write_fmt(fmt),
//         }
//     }
// 
//     fn by_ref(&mut self) -> &mut ConnStream {
//         self
//     }
// }
// 
// impl Write for Conn {
//     fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
//         self.stream.write(buf)
//     }
// 
//     fn flush(&mut self) -> io::Result<()> {
//         self.stream.flush()
//     }
// 
//     fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
//         self.stream.write_all(buf)
//     }
// 
//     fn write_fmt(&mut self, fmt: Arguments) -> io::Result<()> {
//         self.stream.write_fmt(fmt)
//     }
// 
//     fn by_ref(&mut self) -> &mut Conn {
//         self
//     }
// }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_servername_1() {
        let args = vec!["tiny_test".to_owned(),
                        "Your host is adams.freenode.net[94.125.182.252/8001], \
                         running version ircd-seven-1.1.4".to_owned()];
        assert_eq!(parse_servername(&args), Some("adams.freenode.net".to_owned()));
    }

    #[test]
    fn test_parse_servername_2() {
        let args = vec!["Your host is adams.freenode.net[94.125.182.252/8001], \
                         running version ircd-seven-1.1.4".to_owned()];
        assert_eq!(parse_servername(&args), Some("adams.freenode.net".to_owned()));
    }
}
