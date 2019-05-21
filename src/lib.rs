#![cfg_attr(test, feature(test))]
#![feature(allocator_api)]
#![feature(const_fn)]
#![feature(drain_filter)]
#![feature(nll)]
#![feature(ptr_offset_from)]

#[global_allocator]
static ALLOC: std::alloc::System = std::alloc::System;

#[cfg(test)]
extern crate quickcheck;

extern crate dirs;
extern crate futures;
extern crate irc;
extern crate libc;
extern crate mio;
extern crate native_tls;
extern crate net2;
extern crate serde;
extern crate serde_yaml;
extern crate tempfile;
extern crate time;
extern crate tokio;

// extern crate term_input;
extern crate term_input_futures as term_input;
extern crate termbox_simple;

extern crate take_mut;

#[macro_use]
mod utils;

// mod cmd;
mod cmd_line_args;
pub mod config;
// mod conn;
mod logger;
mod notifier;
mod stream;
pub mod trie;
pub mod tui;
mod wire;

use mio::unix::EventedFd;
use mio::unix::UnixReady;
use mio::Events;
use mio::Poll;
use mio::PollOpt;
use mio::Ready;
use mio::Token;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;
use std::time::Instant;

// use cmd::{parse_cmd, ParseCmdResult};
use cmd_line_args::{parse_cmd_line_args, CmdLineArgs};
// use conn::{Conn, ConnErr, ConnEv};
use logger::Logger;
use term_input::{Event, Input};
use tui::{MsgSource, MsgTarget, TUIHandle, TUIRet, TabStyle, Timestamp, TUI};
use wire::{Cmd, Msg, Pfx};

use futures::prelude::*;
use irc::client::ext::ClientExt;
use irc::client::Client;
use irc::client::PackedIrcClient;

////////////////////////////////////////////////////////////////////////////////////////////////////

pub fn run() {
    let CmdLineArgs {
        servers: server_args,
        config_path,
    } = parse_cmd_line_args(std::env::args());
    let config_path = config_path.unwrap_or_else(config::get_default_config_path);
    if config_path.is_dir() {
        println!("The config path is a directory.");
        ::std::process::exit(1);
    } else if !config_path.is_file() {
        config::generate_default_config(&config_path);
    } else {
        match config::parse_config(&config_path) {
            Err(yaml_err) => {
                println!("Can't parse config file:");
                println!("{}", yaml_err);
                ::std::process::exit(1);
            }
            Ok(config::Config {
                servers,
                defaults,
                colors,
                log_dir,
            }) => {
                let servers = if !server_args.is_empty() {
                    // connect only to servers that match at least one of
                    // the given patterns
                    servers
                        .into_iter()
                        .filter(|s| {
                            for server in &server_args {
                                if s.addr.contains(server) {
                                    return true;
                                }
                            }
                            false
                        })
                        .collect()
                } else {
                    servers
                };

                let tui = TUI::new(colors);

                let mentions = tui.create_handle("mentions".to_string());
                mentions.add_client_msg("Any mentions to you will be listed here.");

                tui.draw();

                tokio::runtime::current_thread::run(futures::future::lazy(move || {
                    run_async(servers, defaults, log_dir, config_path, tui)
                }));
            }
        }
    }
}

fn make_irc_config(mut server: config::Server) -> irc::client::data::Config {
    // TODO: We lost SASL support after switching to the irc crate ...
    irc::client::data::Config {
        nickname: Some(server.nicks.remove(0)),
        alt_nicks: Some(server.nicks),
        nick_password: server.nickserv_ident,
        username: Some(server.hostname), // TODO: is this correct?
        realname: Some(server.realname),
        server: Some(server.addr),
        port: Some(server.port),
        use_ssl: Some(server.tls),
        channels: Some(server.join),
        ..irc::client::data::Config::default()
    }
}

// TODO This should show on the TUI
fn report_irc_err(tui: &TUI, error: irc::error::IrcError) {
    eprintln!("Error: {:?}", error);
}

fn report_server_irc_err(tui: &TUIHandle, error: irc::error::IrcError) {
    eprintln!("Error: {:?}", error);
}

// Shared mutable cell of mapping from server names (config.addr) to IrcClients for sending
// messages.
type ConnMap = Rc<RefCell<HashMap<String, Conn>>>;

struct Conn {
    client: irc::client::IrcClient,
    tui_handle: TUIHandle,
    close_signal1: futures::sync::oneshot::Sender<()>,
    close_signal2: futures::sync::oneshot::Sender<()>,
}

#[derive(Debug)]
enum InputErr {
    IoErr(tokio::io::Error),
    Exit,
}

#[derive(Debug)]
enum IrcClientErr {
    IrcErr(irc::error::IrcError),
    ExitSignalled,
}

fn run_async(
    servers: Vec<config::Server>,
    defaults: config::Defaults,
    log_dir: String,
    config_path: PathBuf,
    tui: TUI,
) -> impl Future<Item = (), Error = ()> {
    // TODO: Remove this by passing a "send" callback to the TUI handles so that each tab will know
    // how to send a message.
    let conns: ConnMap = Rc::new(RefCell::new(HashMap::new()));

    // Spawn a task for handling user input
    let tui_clone = tui.clone();
    let input = Input::new();
    let conns_clone1 = conns.clone();
    let conns_clone2 = conns.clone();
    tokio::runtime::current_thread::spawn(
        input
            .map_err(InputErr::IoErr)
            .for_each(
                move |ev| match handle_input_event(ev, &tui_clone, &conns_clone1) {
                    EvLoopRet::Continue => {
                        tui_clone.draw();
                        futures::future::ok(())
                    }
                    EvLoopRet::Break => futures::future::err(InputErr::Exit),
                },
            )
            .map_err(move |err| {
                // match err {
                //     InputErr::Exit => {
                //         for (_, conn) in conns_clone2.borrow_mut().drain() {
                //             conn.close_signal.send(()).unwrap();
                //         }
                //     }
                //     InputErr::IoErr(io_err) => { /* TODO */ }
                // }
                eprintln!("Err: {:?}", err);
                for (_, conn) in conns_clone2.borrow_mut().drain() {
                    conn.close_signal1.send(()).unwrap();
                    conn.close_signal2.send(()).unwrap();
                }
            }),
    );

    // Spawn tasks for connections
    for server in servers {
        let tui_handle1 = tui.create_handle(server.addr.clone());
        let tui_handle2 = tui_handle1.clone();
        let server_name = server.addr.clone();
        let irc_config = make_irc_config(server);

        match irc::client::IrcClient::new_future(irc_config) {
            Err(err) => report_irc_err(&tui, err),
            Ok(f) => {
                let tui_handle2 = tui_handle1.clone();
                let tui_handle3 = tui_handle1.clone();
                let conns_clone = conns.clone();
                tokio::runtime::current_thread::spawn(
                    f.map_err(IrcClientErr::IrcErr)
                        .and_then(move |irc::client::PackedIrcClient(client, future)| {
                            let (snd_close1, rcv_close1) = futures::sync::oneshot::channel();
                            let (snd_close2, rcv_close2) = futures::sync::oneshot::channel();
                            let conn = Conn {
                                client: client.clone(),
                                tui_handle: tui_handle3,
                                close_signal1: snd_close1,
                                close_signal2: snd_close2,
                            };
                            conns_clone.borrow_mut().insert(server_name, conn);
                            // Spawn a task for incoming messages
                            tokio::runtime::current_thread::spawn(handle_incoming_msgs(
                                client,
                                tui_handle1,
                                rcv_close2,
                            ));
                            // Run the connection in the current task
                            future
                                .map_err(IrcClientErr::IrcErr)
                                .select(rcv_close1.map_err(|_| IrcClientErr::ExitSignalled))
                                .map(|(ret, _)| ret)
                                .map_err(|(err, _select_next)| err)
                        })
                        .map_err(move |err| {
                            if let IrcClientErr::IrcErr(err) = err {
                                report_server_irc_err(&tui_handle2, err);
                            }
                        }),
                );
            }
        }
    }

    futures::future::ok(())
}

enum EvLoopRet {
    Continue,
    Break,
}

fn handle_input_event(ev: Event, tui: &TUI, conns: &ConnMap) -> EvLoopRet {
    match tui.handle_input_event(ev) {
        TUIRet::Abort => EvLoopRet::Break,
        TUIRet::KeyHandled | TUIRet::KeyIgnored(_) | TUIRet::EventIgnored(_) => {
            // TODO: Log
            EvLoopRet::Continue
        }
        TUIRet::Input { msg, from } => {
            // TODO: Handle commands
            // TODO: Log
            send_msg(
                &tui,
                &from,
                &msg.into_iter().collect::<String>(),
                conns,
                false,
            );
            EvLoopRet::Continue
        }
        TUIRet::Lines { lines, from } => {
            // TODO: Log
            for line in lines {
                send_msg(&tui, &from, &line, conns, false);
            }
            EvLoopRet::Continue
        }
    }
}

fn send_msg(tui: &TUI, from: &MsgSource, msg: &str, conns: &ConnMap, ctcp_action: bool) {
    // msg_target: Actual PRIVMSG target to send to the server
    // serv_name: Server name to find connection in `conns`
    let (tui_target, msg_target, serv_name) = {
        match from {
            MsgSource::Serv { ref serv_name } => {
                // TODO: Implement sending raw messages. We lost this feature during
                // migration to the irc crate.
                unimplemented!();
            }

            MsgSource::Chan {
                ref serv_name,
                ref chan_name,
            } => (
                MsgTarget::Chan {
                    serv_name,
                    chan_name,
                },
                chan_name,
                serv_name,
            ),

            MsgSource::User {
                ref serv_name,
                ref nick,
            } => {
                let msg_target = if nick.eq_ignore_ascii_case("nickserv")
                    || nick.eq_ignore_ascii_case("chanserv")
                {
                    MsgTarget::Server { serv_name }
                } else {
                    MsgTarget::User { serv_name, nick }
                };
                (msg_target, nick, serv_name)
            }
        }
    };

    let conns_ref = conns.borrow();
    let Conn {
        client, tui_handle, ..
    } = conns_ref.get(serv_name.as_str()).unwrap();

    let send_fn = if ctcp_action {
        irc::client::IrcClient::send_ctcp
    } else {
        irc::client::IrcClient::send_privmsg
    };

    let ts = Timestamp::now();
    send_fn(client, msg_target, msg);

    // FIXME Grabage code. Remove tui_target entirely.
    match tui_target {
        // MsgTarget::Server { .. } => {
        //     tui_handle.add_privmsg_serv(client.current_nickname().to_owned(), msg.to_owned(), ts);
        // }
        MsgTarget::Chan { chan_name, .. } => {
            tui_handle.add_privmsg_chan(
                client.current_nickname().to_owned(),
                msg.to_owned(),
                ts,
                chan_name.to_owned(),
            );
        }
        MsgTarget::User { nick, .. } => {
            tui_handle.add_privmsg_user(
                client.current_nickname().to_owned(),
                msg.to_owned(),
                ts,
                nick.to_owned(),
            );
        }
        _ => panic!(),
    }
}

fn handle_incoming_msgs(
    client: irc::client::IrcClient,
    tui: TUIHandle,
    recv_close: futures::sync::oneshot::Receiver<()>,
) -> impl Future<Item = (), Error = ()> {
    let tui_clone = tui.clone();
    client
        .stream()
        .for_each(move |msg| {
            handle_incoming_msg(msg, client.current_nickname(), &tui);
            futures::future::ok(())
        })
        .map_err(move |err| report_server_irc_err(&tui_clone, err))
        .select(recv_close.map_err(|_| ()))
        .map(|(ret, _)| ret)
        .map_err(|(err, _select_next)| err)
}

fn handle_incoming_msg(
    irc_msg: irc::client::prelude::Message,
    current_nick: &str,
    tui: &TUIHandle,
) {
    use irc::client::prelude::Command::*;
    use irc::client::prelude::Prefix::*;

    match irc_msg.command {
        PRIVMSG(target, msg) => {
            let pfx = match irc_msg.prefix {
                None => {
                    // TODO: log this
                    return;
                }
                Some(pfx) => pfx,
            };

            let origin = match pfx {
                ServerName(serv) => serv,
                Nickname(nick, _username, _hostname) => nick,
            };

            // TODO:
            // - Log
            // - CTCP stuff?
            // - Mentions?

            if target.chars().nth(0) == Some('#') {
                tui.add_privmsg_chan(origin, msg, Timestamp::now(), target);
            } else {
                tui.add_privmsg_user(origin, msg, Timestamp::now(), target);
            }
        }

        JOIN(chanlist, _chankeys, _realname) => {
            let nick = match irc_msg.prefix {
                Some(Nickname(nick, _username, _hostname)) => nick,
                _ => {
                    // TODO: log this
                    return;
                }
            };

            // TODO: Log

            // TODO: chanlist is actually a comma separated list, but most of the time it's
            // just one channel so the code below works most of the time.

            if nick == current_nick {
                tui.new_chan_tab(&chanlist);
            } else {
                let ts = Some(Timestamp::now());
                tui.add_nick_chan(&nick, ts, &chanlist);
                // Also update the private message tab if it exists
                // Nothing will be shown if the user already known to be online by the tab
                if tui.does_user_tab_exist(&nick) {
                    tui.add_nick_user(&nick, ts, &nick);
                }
            }
        }

        PART(chanlist, _comment) => {
            // TODO: Same, chanlist is actually a list

            let nick = match irc_msg.prefix {
                Some(Nickname(nick, _username, _hostname)) => nick,
                _ => {
                    // TODO: log this
                    return;
                }
            };

            if nick != current_nick {
                // TODO: log
                tui.remove_nick_chan(&nick, Some(Timestamp::now()), &chanlist);
            }
        }

        QUIT(_comment) => {
            let nick = match irc_msg.prefix {
                Some(Nickname(nick, _username, _hostname)) => nick,
                _ => {
                    // TODO: log this
                    return;
                }
            };
            tui.remove_nick_all(&nick, Some(Timestamp::now()));
        }

        NICK(new_nick) => {
            let old_nick = match irc_msg.prefix {
                Some(Nickname(nick, _username, _hostname)) => nick,
                _ => {
                    // TODO: log this
                    return;
                }
            };
            tui.rename_nick(&old_nick, &new_nick, Timestamp::now());
        }

        ERROR(error) => {
            tui.add_err_msg(&error, Timestamp::now());
        }

        TOPIC(chan, topic) => {
            if let Some(topic) = topic {
                tui.show_topic(&topic, Timestamp::now(), &chan);
            }
        }

        // TODO: ERR_NICKNAMEINUSE ???
        // TODO: RPL_WELCOME, RPL_YOUHOST, REPL_CREATED, RPS_LUSEROP, RPL_LUSERUNKNOWN,
        //       RPL_LUSERCHANNELS, RPL_TOPIC, RPL_NAMREPLY, RPL_ENDOFNAMES, RPL_UNAWAY,
        //       RPL_NOWAWAY, RPL_AWAY, ERR_NOSUCHNICK ???
        // TODO:  ???
        _ => { /* TODO: LOG THESE */ }
    }
}

////////////////////////////////////////////////////////////////////////////////////////////////////

/*
pub struct Tiny<'poll> {
    conns: Vec<Conn<'poll>>,
    defaults: config::Defaults,
    tui: TUI,
    input_ev_handler: Input,
    logger: Logger,
    config_path: PathBuf,
}
*/

const STDIN_TOKEN: Token = Token(libc::STDIN_FILENO as usize);

/*
impl<'poll> Tiny<'poll> {
    pub fn run(
        servers: Vec<config::Server>,
        defaults: config::Defaults,
        log_dir: String,
        colors: config::Colors,
        config_path: PathBuf,
    ) {
        let poll = Poll::new().unwrap();

        poll.register(
            &EventedFd(&libc::STDIN_FILENO),
            STDIN_TOKEN,
            Ready::readable(),
            PollOpt::level(),
        )
        .unwrap();

        let mut conns = Vec::with_capacity(servers.len());

        let mut tui = TUI::new(colors);

        // init "mentions" tab
        tui.new_server_tab("mentions");
        tui.add_client_msg(
            "Any mentions to you will be listed here.",
            &MsgTarget::Server {
                serv_name: "mentions",
            },
        );

        tui.draw();

        for server in servers.iter().cloned() {
            let msg_target = MsgTarget::Server {
                serv_name: &server.addr.clone(),
            };
            match Conn::new(server, &poll) {
                Ok(conn) => {
                    conns.push(conn);
                }
                Err(err) => {
                    tui.add_err_msg(&connect_err_msg(&err), Timestamp::now(), &msg_target);
                }
            }
        }

        let mut tiny = Tiny {
            conns,
            defaults,
            tui,
            input_ev_handler: Input::new(),
            logger: Logger::new(PathBuf::from(log_dir)),
            config_path: config_path.to_owned(),
        };

        tiny.tui.draw();

        let mut last_tick = Instant::now();
        let mut poll_evs = Events::with_capacity(10);
        let mut conn_evs = Vec::with_capacity(10);
        let mut input_evs = Vec::with_capacity(10);
        'mainloop: loop {
            // FIXME this will sometimes miss the tick deadline
            match poll.poll(&mut poll_evs, Some(Duration::from_secs(1))) {
                Err(_) => {
                    // usually SIGWINCH, which is caught by term_input
                    if tiny.handle_stdin(&poll, &mut input_evs) {
                        break 'mainloop;
                    }
                }
                Ok(_) => {
                    for event in poll_evs.iter() {
                        let token = event.token();
                        if token == STDIN_TOKEN {
                            if tiny.handle_stdin(&poll, &mut input_evs) {
                                break 'mainloop;
                            }
                        } else {
                            match find_token_conn_idx(&tiny.conns, token) {
                                None => {
                                    tiny.logger.get_debug_logs().write_line(format_args!(
                                        "BUG: Can't find Token in conns: {:?}",
                                        event.token()
                                    ));
                                }
                                Some(conn_idx) => {
                                    tiny.handle_socket(event.readiness(), conn_idx, &mut conn_evs);
                                }
                            }
                        }
                    }

                    if last_tick.elapsed() >= Duration::from_secs(1) {
                        for conn_idx in 0..tiny.conns.len() {
                            {
                                let conn = &mut tiny.conns[conn_idx];
                                conn.tick(&mut conn_evs, tiny.logger.get_debug_logs());
                            }
                            tiny.handle_conn_evs(conn_idx, &mut conn_evs);
                        }
                        last_tick = Instant::now();
                    }
                }
            }

            tiny.tui.draw();
        }
    }

    fn handle_stdin(&mut self, poll: &'poll Poll, evs: &mut Vec<Event>) -> bool {
        let mut abort = false;
        self.input_ev_handler.read_input_events(evs);
        for ev in evs.drain(..) {
            match self.tui.handle_input_event(ev) {
                TUIRet::Abort => {
                    abort = true;
                }
                TUIRet::Input { msg, from } => {
                    self.logger.get_debug_logs().write_line(format_args!(
                        "Input source: {:#?}, msg: {}",
                        from,
                        msg.iter().cloned().collect::<String>()
                    ));

                    // We know msg has at least one character as the TUI won't accept it otherwise.
                    if msg[0] == '/' {
                        let msg_str: String = (&msg[1..]).into_iter().cloned().collect();
                        self.handle_cmd(poll, from, &msg_str);
                    } else {
                        self.send_msg(&from, &msg.into_iter().collect::<String>(), false);
                    }
                }
                TUIRet::Lines { lines, from } => {
                    self.logger.get_debug_logs().write_line(format_args!(
                        "Input source: {:#?}, lines: {:?}",
                        from, lines
                    ));
                    for line in lines {
                        self.send_msg(&from, &line, false);
                    }
                }
                TUIRet::KeyHandled => {}
                TUIRet::EventIgnored(Event::FocusGained)
                | TUIRet::EventIgnored(Event::FocusLost) => {}
                ev => {
                    self.logger
                        .get_debug_logs()
                        .write_line(format_args!("Ignoring event: {:?}", ev));
                }
            }
        }
        abort
    }

    fn handle_cmd(&mut self, poll: &'poll Poll, src: MsgSource, msg: &str) {
        match parse_cmd(msg) {
            ParseCmdResult::Ok { cmd, rest } => {
                (cmd.cmd_fn)(rest, poll, self, src);
            }
            // ParseCmdResult::Ambiguous(vec) => {
            //     self.tui.add_client_err_msg(
            //         &format!("Unsupported command: \"/{}\"", msg),
            //         &MsgTarget::CurrentTab,
            //     );
            //     self.tui.add_client_err_msg(
            //         &format!("Did you mean one of {:?} ?", vec),
            //         &MsgTarget::CurrentTab,
            //     );
            // },
            ParseCmdResult::Unknown => self.tui.add_client_err_msg(
                &format!("Unsupported command: \"/{}\"", msg),
                &MsgTarget::CurrentTab,
            ),
        }
    }

    fn part(&mut self, serv_name: &str, chan: &str) {
        let conn = find_conn(&mut self.conns, serv_name).unwrap();
        conn.part(chan);
    }

    fn send_msg(&mut self, from: &MsgSource, msg: &str, ctcp_action: bool) {
        if from.serv_name() == "mentions" {
            self.tui.add_client_err_msg(
                "Use `/connect <server>` to connect to a server",
                &MsgTarget::CurrentTab,
            );
            return;
        }

        // `tui_target`: Where to show the message on TUI
        // `msg_target`: Actual PRIVMSG target to send to the server
        // `serv_name`: Server name to find connection in `self.conns`
        let (tui_target, msg_target, serv_name) = {
            match from {
                MsgSource::Serv { ref serv_name } => {
                    // we don't split raw messages to 512-bytes long chunks
                    if let Some(conn) = self
                        .conns
                        .iter_mut()
                        .find(|conn| conn.get_serv_name() == serv_name)
                    {
                        conn.raw_msg(msg);
                    } else {
                        self.tui.add_client_err_msg(
                            &format!("Can't find server: {}", serv_name),
                            &MsgTarget::CurrentTab,
                        );
                    }
                    return;
                }

                MsgSource::Chan {
                    ref serv_name,
                    ref chan_name,
                } => (
                    MsgTarget::Chan {
                        serv_name,
                        chan_name,
                    },
                    chan_name,
                    serv_name,
                ),

                MsgSource::User {
                    ref serv_name,
                    ref nick,
                } => {
                    let msg_target = if nick.eq_ignore_ascii_case("nickserv")
                        || nick.eq_ignore_ascii_case("chanserv")
                    {
                        MsgTarget::Server { serv_name }
                    } else {
                        MsgTarget::User { serv_name, nick }
                    };
                    (msg_target, nick, serv_name)
                }
            }
        };

        let conn = find_conn(&mut self.conns, serv_name).unwrap();
        let ts = Timestamp::now();
        let extra_len = msg_target.len() as i32
            + if ctcp_action {
                9 // "\0x1ACTION \0x1".len()
            } else {
                0
            };
        let send_fn = if ctcp_action {
            Conn::ctcp_action
        } else {
            Conn::privmsg
        };
        for msg in conn.split_privmsg(extra_len, msg) {
            send_fn(conn, msg_target, msg);
            self.tui
                .add_privmsg(conn.get_nick(), msg, ts, &tui_target, ctcp_action);
        }
    }

    ////////////////////////////////////////////////////////////////////////////

    fn handle_socket(&mut self, readiness: Ready, conn_idx: usize, evs: &mut Vec<ConnEv>) {
        if readiness.is_readable() {
            self.conns[conn_idx].read_ready(evs, &mut self.logger);
        }
        // Handle `ConnEv`s first before checking other readiness events. Reason: we sometimes
        // realize that the connection is closed/got broken at this point even though a write
        // readiness event is also available. When this happens we need to enter disconnect state
        // first, otherwise we end up calling `write_ready()` on a broken/disconnected `Stream`,
        // which causes a panic. This caused #119.
        self.handle_conn_evs(conn_idx, evs);
        // This does nothing if we entered disconnect state in the line above.
        if readiness.is_writable() {
            self.conns[conn_idx].write_ready(evs);
        }
        if readiness.contains(UnixReady::hup()) {
            self.conns[conn_idx].enter_disconnect_state();
            self.tui.add_err_msg(
                &format!(
                    "Connection error (HUP). \
                     Will try to reconnect in {} seconds.",
                    conn::RECONNECT_TICKS
                ),
                Timestamp::now(),
                &MsgTarget::AllServTabs {
                    serv_name: self.conns[conn_idx].get_serv_name(),
                },
            );
        }
        self.handle_conn_evs(conn_idx, evs);
    }

    fn handle_conn_evs(&mut self, conn_idx: usize, evs: &mut Vec<ConnEv>) {
        for ev in evs.drain(..) {
            self.handle_conn_ev(conn_idx, ev);
        }
    }

    fn handle_conn_ev(&mut self, conn_idx: usize, ev: ConnEv) {
        match ev {
            ConnEv::Connected => {
                self.tui.add_msg(
                    "Connected.",
                    Timestamp::now(),
                    &MsgTarget::AllServTabs {
                        serv_name: self.conns[conn_idx].get_serv_name(),
                    },
                );
            }
            ConnEv::Disconnected => {
                let conn = &mut self.conns[conn_idx];
                let target = MsgTarget::AllServTabs {
                    serv_name: conn.get_serv_name(),
                };
                self.tui.add_err_msg(
                    &format!(
                        "Disconnected. Will try to reconnect in {} seconds.",
                        conn::RECONNECT_TICKS
                    ),
                    Timestamp::now(),
                    &target,
                );
                self.tui.clear_nicks(&target);
            }
            ConnEv::WantReconnect => {
                let conn = &mut self.conns[conn_idx];
                self.tui.add_client_msg(
                    "Connecting...",
                    &MsgTarget::AllServTabs {
                        serv_name: conn.get_serv_name(),
                    },
                );
                match conn.reconnect(None) {
                    Ok(()) => {}
                    Err(err) => {
                        self.tui.add_err_msg(
                            &reconnect_err_msg(&err),
                            Timestamp::now(),
                            &MsgTarget::AllServTabs {
                                serv_name: conn.get_serv_name(),
                            },
                        );
                    }
                }
            }
            ConnEv::Err(err) => {
                let conn = &mut self.conns[conn_idx];
                conn.enter_disconnect_state();
                self.tui.add_err_msg(
                    &reconnect_err_msg(&err),
                    Timestamp::now(),
                    &MsgTarget::AllServTabs {
                        serv_name: conn.get_serv_name(),
                    },
                );
            }
            ConnEv::Msg(msg) => {
                self.handle_msg(conn_idx, msg, Timestamp::now());
            }
            ConnEv::NickChange(new_nick) => {
                let conn = &self.conns[conn_idx];
                self.tui.set_nick(conn.get_serv_name(), &new_nick);
            }
        }
    }

    fn handle_msg(&mut self, conn_idx: usize, msg: Msg, ts: Timestamp) {
        let conn = &self.conns[conn_idx];
        let pfx = msg.pfx;
        match msg.cmd {
            Cmd::PRIVMSG {
                target,
                msg,
                is_notice,
            } => {
                let pfx = match pfx {
                    Some(pfx) => pfx,
                    None => {
                        self.logger.get_debug_logs().write_line(format_args!(
                            "PRIVMSG or NOTICE without prefix \
                             target: {:?} msg: {:?}",
                            target, msg
                        ));
                        return;
                    }
                };

                // sender to be shown in the UI
                let origin = match pfx {
                    Pfx::Server(_) => conn.get_serv_name(),
                    Pfx::User { ref nick, .. } => nick,
                };

                let (msg, is_ctcp_action) = wire::check_ctcp_action_msg(&msg);

                match target {
                    wire::MsgTarget::Chan(chan) => {
                        self.logger
                            .get_chan_logs(conn.get_serv_name(), &chan)
                            .write_line(format_args!("PRIVMSG: {}", msg));
                        let msg_target = MsgTarget::Chan {
                            serv_name: conn.get_serv_name(),
                            chan_name: &chan,
                        };
                        // highlight the message if it mentions us
                        if msg.find(conn.get_nick()).is_some() {
                            self.tui.add_privmsg_highlight(
                                origin,
                                msg,
                                ts,
                                &msg_target,
                                is_ctcp_action,
                            );
                            self.tui.set_tab_style(TabStyle::Highlight, &msg_target);
                            let mentions_target = MsgTarget::Server {
                                serv_name: "mentions",
                            };
                            self.tui.add_msg(
                                &format!(
                                    "{} in {}:{}: {}",
                                    origin,
                                    conn.get_serv_name(),
                                    chan,
                                    msg
                                ),
                                ts,
                                &mentions_target,
                            );
                            self.tui
                                .set_tab_style(TabStyle::Highlight, &mentions_target);
                        } else {
                            self.tui
                                .add_privmsg(origin, msg, ts, &msg_target, is_ctcp_action);
                            self.tui.set_tab_style(TabStyle::NewMsg, &msg_target);
                        }
                    }
                    wire::MsgTarget::User(target) => {
                        let serv_name = conn.get_serv_name();
                        let msg_target = {
                            match pfx {
                                Pfx::Server(_) => MsgTarget::Server { serv_name },
                                Pfx::User { ref nick, .. } => {
                                    // show NOTICE messages in server tabs if we don't have a tab
                                    // for the sender already (see #21)
                                    if is_notice && !self.tui.does_user_tab_exist(serv_name, nick) {
                                        MsgTarget::Server { serv_name }
                                    } else {
                                        MsgTarget::User { serv_name, nick }
                                    }
                                }
                            }
                        };
                        self.tui
                            .add_privmsg(origin, msg, ts, &msg_target, is_ctcp_action);
                        if target == conn.get_nick() {
                            self.tui.set_tab_style(TabStyle::Highlight, &msg_target);
                        } else {
                            // not sure if this case can happen
                            self.tui.set_tab_style(TabStyle::NewMsg, &msg_target);
                        }
                    }
                }
            }

            Cmd::JOIN { chan } => match pfx {
                Some(Pfx::User { nick, .. }) => {
                    let serv_name = conn.get_serv_name();
                    self.logger
                        .get_chan_logs(serv_name, &chan)
                        .write_line(format_args!("JOIN: {}", nick));
                    if nick == conn.get_nick() {
                        self.tui.new_chan_tab(serv_name, &chan);
                    } else {
                        let nick = drop_nick_prefix(&nick);
                        let ts = Some(Timestamp::now());
                        self.tui.add_nick(
                            nick,
                            ts,
                            &MsgTarget::Chan {
                                serv_name,
                                chan_name: &chan,
                            },
                        );
                        // Also update the private message tab if it exists
                        // Nothing will be shown if the user already known to be online by the
                        // tab
                        if self.tui.does_user_tab_exist(serv_name, nick) {
                            self.tui
                                .add_nick(nick, ts, &MsgTarget::User { serv_name, nick });
                        }
                    }
                }
                pfx => {
                    self.logger
                        .get_debug_logs()
                        .write_line(format_args!("Weird JOIN message pfx {:?}", pfx));
                }
            },

            Cmd::PART { chan, .. } => match pfx {
                Some(Pfx::User { nick, .. }) => {
                    if nick != conn.get_nick() {
                        let serv_name = conn.get_serv_name();
                        self.logger
                            .get_chan_logs(serv_name, &chan)
                            .write_line(format_args!("PART: {}", nick));
                        self.tui.remove_nick(
                            &nick,
                            Some(Timestamp::now()),
                            &MsgTarget::Chan {
                                serv_name,
                                chan_name: &chan,
                            },
                        );
                    }
                }
                pfx => {
                    self.logger
                        .get_debug_logs()
                        .write_line(format_args!("Weird PART message pfx {:?}", pfx));
                }
            },

            Cmd::QUIT { .. } => match pfx {
                Some(Pfx::User { ref nick, .. }) => {
                    let serv_name = conn.get_serv_name();
                    self.tui.remove_nick(
                        nick,
                        Some(Timestamp::now()),
                        &MsgTarget::AllUserTabs { serv_name, nick },
                    );
                }
                pfx => {
                    self.logger
                        .get_debug_logs()
                        .write_line(format_args!("Weird QUIT message pfx {:?}", pfx));
                }
            },

            Cmd::NICK { nick } => match pfx {
                Some(Pfx::User {
                    nick: ref old_nick, ..
                }) => {
                    let serv_name = conn.get_serv_name();
                    self.tui.rename_nick(
                        old_nick,
                        &nick,
                        Timestamp::now(),
                        &MsgTarget::AllUserTabs {
                            serv_name,
                            nick: old_nick,
                        },
                    );
                }
                pfx => {
                    self.logger
                        .get_debug_logs()
                        .write_line(format_args!("Weird NICK message pfx {:?}", pfx));
                }
            },

            Cmd::Reply { num: 433, .. } => {
                // ERR_NICKNAMEINUSE
                if conn.is_nick_accepted() {
                    // Nick change request from user failed. Just show an error message.
                    self.tui.add_err_msg(
                        "Nickname is already in use",
                        Timestamp::now(),
                        &MsgTarget::AllServTabs {
                            serv_name: conn.get_serv_name(),
                        },
                    );
                }
            }

            Cmd::PING { .. } | Cmd::PONG { .. } =>
                // ignore
                {}

            Cmd::ERROR { ref msg } => {
                let serv_name = conn.get_serv_name();
                self.tui
                    .add_err_msg(msg, Timestamp::now(), &MsgTarget::AllServTabs { serv_name });
            }

            Cmd::TOPIC {
                ref chan,
                ref topic,
            } => {
                self.tui.show_topic(
                    topic,
                    Timestamp::now(),
                    &MsgTarget::Chan {
                        serv_name: conn.get_serv_name(),
                        chan_name: chan,
                    },
                );
            }

            Cmd::CAP {
                client: _,
                ref subcommand,
                ref params,
            } => {
                match subcommand.as_ref() {
                    "NAK" => {
                        if params.iter().any(|cap| cap.as_str() == "sasl") {
                            let msg_target = MsgTarget::Server {
                                serv_name: conn.get_serv_name(),
                            };
                            self.tui.add_err_msg(
                                "Server rejected using SASL authenication capability",
                                Timestamp::now(),
                                &msg_target,
                            );
                        }
                    }
                    "LS" => {
                        if !params.iter().any(|cap| cap.as_str() == "sasl") {
                            let msg_target = MsgTarget::Server {
                                serv_name: conn.get_serv_name(),
                            };
                            self.tui.add_err_msg(
                                "Server does not support SASL authenication",
                                Timestamp::now(),
                                &msg_target,
                            );
                        }
                    }
                    "ACK" => {}
                    cmd => {
                        self.logger
                            .get_debug_logs()
                            .write_line(format_args!("CAP subcommand {} is not handled", cmd));
                    }
                };
            }

            Cmd::AUTHENTICATE { .. } =>
                // ignore
                {}

            Cmd::Reply { num: n, params } => {
                if n <= 003 /* RPL_WELCOME, RPL_YOURHOST, RPL_CREATED */
                        || n == 251 /* RPL_LUSERCLIENT */
                        || n == 255 /* RPL_LUSERME */
                        || n == 372 /* RPL_MOTD */
                        || n == 375 /* RPL_MOTDSTART */
                        || n == 376
                /* RPL_ENDOFMOTD */
                {
                    debug_assert_eq!(params.len(), 2);
                    let msg = &params[1];
                    self.tui.add_msg(
                        msg,
                        Timestamp::now(),
                        &MsgTarget::Server {
                            serv_name: conn.get_serv_name(),
                        },
                    );
                } else if n == 4 // RPL_MYINFO
                        || n == 5 // RPL_BOUNCE
                        || (n >= 252 && n <= 254)
                /* RPL_LUSEROP, RPL_LUSERUNKNOWN, */
                /* RPL_LUSERCHANNELS */
                {
                    let msg = params.into_iter().collect::<Vec<String>>().join(" ");
                    self.tui.add_msg(
                        &msg,
                        Timestamp::now(),
                        &MsgTarget::Server {
                            serv_name: conn.get_serv_name(),
                        },
                    );
                } else if n == 265 || n == 266 || n == 250 {
                    let msg = &params[params.len() - 1];
                    self.tui.add_msg(
                        msg,
                        Timestamp::now(),
                        &MsgTarget::Server {
                            serv_name: conn.get_serv_name(),
                        },
                    );
                }
                // RPL_TOPIC
                else if n == 332 {
                    // FIXME: RFC 2812 says this will have 2 arguments, but freenode
                    // sends 3 arguments (extra one being our nick).
                    assert!(params.len() == 3 || params.len() == 2);
                    let chan = &params[params.len() - 2];
                    let topic = &params[params.len() - 1];
                    self.tui.show_topic(
                        topic,
                        Timestamp::now(),
                        &MsgTarget::Chan {
                            serv_name: conn.get_serv_name(),
                            chan_name: chan,
                        },
                    );
                }
                // RPL_NAMREPLY: List of users in a channel
                else if n == 353 {
                    let chan = &params[2];
                    let chan_target = MsgTarget::Chan {
                        serv_name: conn.get_serv_name(),
                        chan_name: chan,
                    };

                    for nick in params[3].split_whitespace() {
                        self.tui
                            .add_nick(drop_nick_prefix(nick), None, &chan_target);
                    }
                }
                // RPL_ENDOFNAMES: End of NAMES list
                else if n == 366 {
                }
                // RPL_UNAWAY or RPL_NOWAWAY
                else if n == 305 || n == 306 {
                    let msg = &params[1];
                    self.tui.add_client_msg(
                        msg,
                        &MsgTarget::AllServTabs {
                            serv_name: conn.get_serv_name(),
                        },
                    );
                }
                // ERR_NOSUCHNICK
                else if n == 401 {
                    let nick = &params[1];
                    let msg = &params[2];
                    let serv_name = conn.get_serv_name();
                    self.tui
                        .add_client_msg(msg, &MsgTarget::User { serv_name, nick });
                // RPL_AWAY
                } else if n == 301 {
                    let serv_name = conn.get_serv_name();
                    let nick = &params[1];
                    let msg = &params[2];
                    self.tui.add_client_msg(
                        &format!("{} is away: {}", nick, msg),
                        &MsgTarget::User { serv_name, nick },
                    );
                } else {
                    match pfx {
                        Some(Pfx::Server(msg_serv_name)) => {
                            let conn_serv_name = conn.get_serv_name();
                            let msg_target = MsgTarget::Server {
                                serv_name: conn_serv_name,
                            };
                            self.tui.add_privmsg(
                                &msg_serv_name,
                                &params.join(" "),
                                Timestamp::now(),
                                &msg_target,
                                false,
                            );
                            self.tui.set_tab_style(TabStyle::NewMsg, &msg_target);
                        }
                        pfx => {
                            // add everything else to debug file
                            self.logger.get_debug_logs().write_line(format_args!(
                                "Ignoring numeric reply msg:\nPfx: {:?}, num: {:?}, args: {:?}",
                                pfx, n, params
                            ));
                        }
                    }
                }
            }

            Cmd::Other { cmd, params } => match pfx {
                Some(Pfx::Server(msg_serv_name)) => {
                    let conn_serv_name = conn.get_serv_name();
                    let msg_target = MsgTarget::Server {
                        serv_name: conn_serv_name,
                    };
                    self.tui.add_privmsg(
                        &msg_serv_name,
                        &params.join(" "),
                        Timestamp::now(),
                        &msg_target,
                        false,
                    );
                    self.tui.set_tab_style(TabStyle::NewMsg, &msg_target);
                }
                pfx => {
                    self.logger.get_debug_logs().write_line(format_args!(
                        "Ignoring msg:\nPfx: {:?}, msg: {} :{}",
                        pfx,
                        cmd,
                        params.join(" "),
                    ));
                }
            },
        }
    }
}
*/

/*
fn find_token_conn_idx(conns: &[Conn], token: Token) -> Option<usize> {
    for (conn_idx, conn) in conns.iter().enumerate() {
        if conn.get_conn_tok() == Some(token) {
            return Some(conn_idx);
        }
    }
    None
}

fn find_conn<'a, 'poll>(
    conns: &'a mut [Conn<'poll>],
    serv_name: &str,
) -> Option<&'a mut Conn<'poll>> {
    match find_conn_idx(conns, serv_name) {
        None => None,
        Some(idx) => Some(&mut conns[idx]),
    }
}

fn find_conn_idx(conns: &[Conn], serv_name: &str) -> Option<usize> {
    for (conn_idx, conn) in conns.iter().enumerate() {
        if conn.get_serv_name() == serv_name {
            return Some(conn_idx);
        }
    }
    None
}

fn connect_err_msg(err: &ConnErr) -> String {
    match err.source() {
        Some(other_err) => format!(
            "Connection error: {} ({})",
            err.description(),
            other_err.description()
        ),
        None => format!("Connection error: {}", err.description()),
    }
}

fn reconnect_err_msg(err: &ConnErr) -> String {
    match err.source() {
        Some(other_err) => format!(
            "Connection error: {} ({}). \
             Will try to reconnect in {} seconds.",
            err.description(),
            other_err.description(),
            conn::RECONNECT_TICKS
        ),
        None => format!(
            "Connection error: {}. \
             Will try to reconnect in {} seconds.",
            err.description(),
            conn::RECONNECT_TICKS
        ),
    }
}
*/

/// Nicks may have prefixes, indicating it is a operator, founder, or
/// something else.
/// Channel Membership Prefixes:
/// http://modern.ircdocs.horse/#channel-membership-prefixes
///
/// Returns the nick without prefix
fn drop_nick_prefix(nick: &str) -> &str {
    static PREFIXES: [char; 5] = ['~', '&', '@', '%', '+'];

    if PREFIXES.contains(&nick.chars().nth(0).unwrap()) {
        &nick[1..]
    } else {
        nick
    }
}
