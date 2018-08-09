use conn::{Conn, ConnEv};
use wire::{Cmd, Msg, Pfx, PrivMsgTarget};

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

pub struct Logger {
    // All log files will be created in this directory
    log_dir_path: PathBuf,

    // We need a hierarchy here to be able to easily implement things like "add this line to all
    // server log files".
    servers: HashMap<String, ServerLogs>,

    // File for debug logs
    debug_file: Option<File>,
}

struct ServerLogs {
    // Copying this path here to avoid borrowchk issues (sigh)
    log_dir_path: PathBuf,

    server_name: String,

    // File for messages from the server
    server_file: Option<File>,

    // Files for channels or privmsgs
    chan_files: HashMap<String, File>,
}

impl Logger {
    pub fn new(log_dir_path: PathBuf) -> Logger {
        Logger {
            log_dir_path,
            servers: HashMap::new(),
            debug_file: None,
        }
    }
}

//////////////////////////////
// Getting log file handles //
//////////////////////////////

// TODO: We should close handles for closed channels/servers/privmsgs

// TODO (when nll gets smarter): Lots of stupid code below because "conservative nll" is not smart
// enough. Two main problems:
//   - HashMaps
//   - Can't borrow field x via a method and borrow another field y

impl ServerLogs {
    fn get_server_file(&mut self) -> io::Result<&File> {
        match self.server_file {
            Some(ref file) => Ok(file),
            None => {
                let mut path = self.log_dir_path.clone();
                path.push(format!("{}.txt", self.server_name));
                match OpenOptions::new().append(true).create(true).open(path) {
                    Ok(file) => {
                        self.server_file = Some(file);
                        self.get_server_file()
                    }
                    Err(err) => Err(err),
                }
            }
        }
    }

    fn get_chan_file(&mut self, chan: &str) -> io::Result<&File> {
        if self.chan_files.get(chan).is_some() {
            Ok(self.chan_files.get(chan).unwrap())
        } else {
            let mut path = self.log_dir_path.clone();
            path.push(format!("{}_{}.txt", self.server_name, chan));
            match OpenOptions::new().append(true).create(true).open(path) {
                Ok(file) => {
                    self.chan_files.insert(chan.to_owned(), file);
                    Ok(self.chan_files.get(chan).unwrap())
                }
                Err(err) => Err(err),
            }
        }
    }

    fn get_privmsg_file(&mut self, nick: &str) -> io::Result<&File> {
        self.get_chan_file(nick)
    }

    fn get_all_files<'a>(&'a mut self) -> Box<Iterator<Item = &mut File> + 'a> {
        if let Some(ref mut serv_file) = self.server_file {
            Box::new(::std::iter::once(serv_file).chain(self.chan_files.values_mut()))
        } else {
            Box::new(self.chan_files.values_mut())
        }
    }
}

impl Logger {
    fn get_debug_file(&mut self) -> io::Result<&File> {
        match self.debug_file {
            Some(ref file) => Ok(file),
            None => {
                let mut path = self.log_dir_path.clone();
                path.push("debug.txt");
                match OpenOptions::new().append(true).create(true).open(path) {
                    Ok(file) => {
                        self.debug_file = Some(file);
                        Ok(self.debug_file.as_ref().unwrap())
                    }
                    Err(err) => Err(err),
                }
            }
        }
    }

    fn get_server_logs(&mut self, server: &str) -> &mut ServerLogs {
        if self.servers.get(server).is_some() {
            self.servers.get_mut(server).unwrap()
        } else {
            self.servers.insert(
                server.to_owned(),
                ServerLogs {
                    log_dir_path: self.log_dir_path.clone(),
                    server_name: server.to_owned(),
                    server_file: None,
                    chan_files: HashMap::new(),
                },
            );
            self.servers.get_mut(server).unwrap()
        }
    }

    fn get_server_file(&mut self, server: &str) -> io::Result<&File> {
        self.get_server_logs(server).get_server_file()
    }

    fn get_chan_file(&mut self, server: &str, chan: &str) -> io::Result<&File> {
        self.get_server_logs(server).get_chan_file(chan)
    }

    fn get_privmsg_file(&mut self, server: &str, nick: &str) -> io::Result<&File> {
        self.get_server_logs(server).get_privmsg_file(nick)
    }

    fn get_all_server_files<'a>(
        &'a mut self,
        server: &str,
    ) -> Box<Iterator<Item = &mut File> + 'a> {
        self.get_server_logs(server).get_all_files()
    }
}

////////////////
// Debug logs //
////////////////

impl Logger {
    pub fn debug(&mut self, msg: &str) {
        if let Ok(ref mut file) = self.get_debug_file() {
            log(file, msg);
        }
    }
}

/////////////////////////
// Logging conn events //
/////////////////////////

use std::fmt::Arguments;
use std::io::Write;
use time;

fn log(output: &mut Write, msg: &str) {
    let now = time::now();
    let _ = write!(output, "[{}] ", now.rfc822()).and_then(|()| write!(output, "{}", msg));
}

impl Logger {
    pub fn log_event(&mut self, conn: &Conn, ev: &ConnEv) {
        use ConnEv::*;

        match ev {
            Connected => {
                if let Ok(ref mut file) = self.get_server_file(conn.get_serv_name()) {
                    log(file, "** Connected to the server\n");
                }
            }
            Disconnected => {
                if let Ok(ref mut file) = self.get_server_file(conn.get_serv_name()) {
                    log(file, "** Disconnected\n");
                }
            }
            WantReconnect => {
                // Ignore
            }
            Err(err) => {
                // Log connection error to all tabs of the server
                let msg = format!("** Connection error: {:?}\n", err);
                for file in self.get_all_server_files(conn.get_serv_name()) {
                    log(file, &msg);
                }
            }
            Msg(ref msg) => {
                self.log_msg(conn, msg);
            }
            NickChange(new_nick) => {
                // Add a line about the nick change in all tabs of the server
                let msg = format!("** Nick changed to {}\n", new_nick);
                for file in self.get_all_server_files(conn.get_serv_name()) {
                    log(file, &msg);
                }
            }
        }
    }

    fn log_msg(&mut self, conn: &Conn, msg: &Msg) {
        let Msg { ref pfx, ref cmd } = msg;

        use Cmd::*;

        let pfx = match pfx {
            Some(pfx) => pfx,
            None => {
                // A message without prefix. I don't understand how this can happen.
                // Log to debug file.
                if let Ok(ref mut file) = self.get_debug_file() {
                    log(file, &format!("Message without prefix: {:?}\n", msg));
                }
                return;
            }
        };

        match cmd {
            PRIVMSG {
                target,
                msg,
                is_notice,
            } => {
                let mut file = match target {
                    PrivMsgTarget::Chan(chan) => self.get_chan_file(conn.get_serv_name(), chan),
                    PrivMsgTarget::User(nick) => self.get_privmsg_file(conn.get_serv_name(), nick),
                };

                let sender = match pfx {
                    Pfx::Server(ref serv) => serv,
                    Pfx::User { ref nick, .. } => nick,
                };

                if let Ok(ref mut file) = file {
                    // TODO: maybe show is_notice differently
                    log(file, &format!("{}: {}\n", sender, msg));
                }
            }

            _ => {}
        }
    }
}

///////////////////////////////
// Logging outgoing messages //
///////////////////////////////

impl Logger {
    pub fn log_server_msg(&mut self, server: &str, nick: &str, msg: &str) {
        if let Ok(ref mut file) = self.get_server_file(server) {
            log(file, &format!("{}: {}\n", nick, msg));
        }
    }
}
