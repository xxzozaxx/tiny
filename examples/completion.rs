
extern crate libc;
extern crate term_input;
extern crate termbox_simple;
extern crate time;
extern crate tiny;

use term_input::{Input, Event};

use std::fs::File;
use std::io::Read;
use std::io::Write;
use std::mem;

use tiny::tui::{TUI, TUIRet, MsgTarget};

fn loop_() -> Option<String> {
    let mut tui = TUI::new();
    tui.new_server_tab("debug");

    writeln!(tui, "Loading word list for auto-completion ...").unwrap();
    tui.draw();

    {
        let mut contents = String::new();
        let mut file = File::open("/usr/share/dict/american").unwrap();
        file.read_to_string(&mut contents).unwrap();
        for word in contents.lines() {
            tui.add_nick(word, None, &MsgTarget::Server { serv_name: "debug" });
        }
    }

    writeln!(tui, "Done.").unwrap();
    tui.draw();

    let mut fd_set : libc::fd_set = unsafe { mem::zeroed() };
    unsafe { libc::FD_SET(libc::STDIN_FILENO, &mut fd_set); }
    let nfds = libc::STDIN_FILENO + 1;

    let mut input = Input::new();
    let mut ev_buffer : Vec<Event> = Vec::new();

    loop {
        tui.draw();

        let mut fd_set_ = fd_set.clone();
        let ret = unsafe {
            libc::select(nfds,
                         &mut fd_set_,           // read fds
                         std::ptr::null_mut(),   // write fds
                         std::ptr::null_mut(),   // error fds
                         std::ptr::null_mut())   // timeval
        };

        if unsafe { ret == -1 || libc::FD_ISSET(libc::STDIN_FILENO, &mut fd_set_) } {
            input.read_input_events(&mut ev_buffer);
            for ev in ev_buffer.drain(0..) {
                match tui.handle_input_event(ev) {
                    TUIRet::Input { msg, .. } => {
                        tui.add_msg(&msg.into_iter().collect::<String>(),
                                    &time::now(),
                                    &MsgTarget::Server { serv_name: "debug" });
                    },
                    TUIRet::Abort => {
                        return None;
                    },
                    _ => {}
                }
            }
        }
    }
}

fn main() {
    loop_().map(|err| println!("{}", err));
}
