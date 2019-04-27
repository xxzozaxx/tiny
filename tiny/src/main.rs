#![feature(ptr_offset_from)]

extern crate iny;
extern crate term_input;
extern crate termbox_simple;
extern crate time;

mod config;
mod exit_dialogue;
mod messaging;
mod msg_area;
mod tab;
mod termbox;
mod text_field;
mod trie;
mod utils;
mod widget;

use self::config::Colors;
use self::tab::Tab;
use iny::notifier::Notifier;
use iny::ui::*;
use messaging::MessagingUI;
use tab::TabStyle;
use term_input::Event;
use termbox_simple::Termbox;
use time::Tm;

struct Tiny {
    tb: Termbox,
    colors: Colors,
    tabs: Vec<Tab>,
    active_idx: usize,
    width: i32,
    height: i32,
    h_scroll: i32,
}

impl UI for Tiny {
    type InputEvent = Event;

    fn new_server_tab(&mut self, serv_name: &str) {
        self.new_server_tab_(serv_name);
    }

    fn close_server_tab(&mut self, serv_name: &str) {
        if let Some(tab_idx) = self.find_serv_tab_idx(serv_name) {
            self.tabs
                .retain(|tab: &Tab| tab.src.serv_name() != serv_name);
            if self.active_idx == tab_idx {
                self.select_tab(if tab_idx == 0 { 0 } else { tab_idx - 1 });
            }
        }
    }

    fn new_chan_tab(&self, serv_name: &str, chan_name: &str) {
        unimplemented!()
    }

    fn close_chan_tab(&self, serv_name: &str, chan_name: &str) {
        unimplemented!()
    }

    fn new_user_tab(&self, serv_name: &str, nick: &str) {
        unimplemented!()
    }

    fn close_user_tab(&self, serv_name: &str, nick: &str) {
        unimplemented!()
    }

    fn handle_input_event(&self, ev: Self::InputEvent) {
        unimplemented!()
    }

    fn resize(&self) {
        unimplemented!()
    }

    fn switch(&self, string: &str) {
        unimplemented!()
    }

    fn add_client_err_msg(&mut self, msg: &str, target: &MsgTarget) {
        unimplemented!()
    }

    fn add_client_notify_msg(&mut self, msg: &str, target: &MsgTarget) {
        unimplemented!()
    }

    fn add_client_msg(&mut self, msg: &str, target: &MsgTarget) {
        unimplemented!()
    }

    fn add_privmsg(
        &mut self,
        sender: &str,
        msg: &str,
        ts: Tm,
        target: &MsgTarget,
        ctcp_action: bool,
    ) {
        unimplemented!()
    }

    fn add_privmsg_highlight(
        &mut self,
        sender: &str,
        msg: &str,
        ts: Tm,
        target: &MsgTarget,
        ctcp_action: bool,
    ) {
        unimplemented!()
    }

    fn add_msg(&mut self, msg: &str, ts: Tm, target: &MsgTarget) {
        unimplemented!()
    }

    fn add_err_msg(&mut self, msg: &str, ts: Tm, target: &MsgTarget) {
        unimplemented!()
    }

    fn show_topic(&mut self, title: &str, ts: Tm, target: &MsgTarget) {
        unimplemented!()
    }

    fn clear_nicks(&mut self, target: &MsgTarget) {
        unimplemented!()
    }

    fn add_nick(&mut self, nick: &str, ts: Option<Tm>, target: &MsgTarget) {
        unimplemented!()
    }

    fn remove_nick(&mut self, nick: &str, ts: Option<Tm>, target: &MsgTarget) {
        unimplemented!()
    }

    fn rename_nick(&mut self, old_nick: &str, new_nick: &str, ts: Tm, target: &MsgTarget) {
        unimplemented!()
    }

    fn set_nick(&mut self, serv_name: &str, new_nick: &str) {
        unimplemented!()
    }

    fn clear(&mut self, target: &MsgTarget) {
        unimplemented!()
    }

    fn toggle_ignore(&mut self, target: &MsgTarget) {
        unimplemented!()
    }

    fn does_user_tab_exist(&self, serv_name_: &str, nick_: &str) -> bool {
        unimplemented!()
    }
}

impl Tiny {
    fn new_server_tab_(&mut self, serv_name: &str) -> Option<usize> {
        match self.find_serv_tab_idx(serv_name) {
            None => {
                let tab_idx = self.tabs.len();
                self.new_tab(
                    tab_idx,
                    MsgSource::Serv {
                        serv_name: serv_name.to_owned(),
                    },
                    true,
                    Notifier::Mentions,
                );
                Some(tab_idx)
            }
            Some(_) => None,
        }
    }

    fn new_tab(&mut self, idx: usize, src: MsgSource, status: bool, notifier: Notifier) {
        use std::collections::HashMap;

        let mut switch_keys: HashMap<char, i8> = HashMap::with_capacity(self.tabs.len());
        for tab in &self.tabs {
            if let Some(key) = tab.switch {
                switch_keys.entry(key).and_modify(|e| *e += 1).or_insert(1);
            }
        }

        let switch = {
            let mut ret = None;
            let mut n = 0;
            for ch in src.visible_name().chars() {
                if !ch.is_alphabetic() {
                    continue;
                }
                match switch_keys.get(&ch) {
                    None => {
                        ret = Some(ch);
                        break;
                    }
                    Some(n_) => {
                        if ret == None || n > *n_ {
                            ret = Some(ch);
                            n = *n_;
                        }
                    }
                }
            }
            ret
        };

        self.tabs.insert(
            idx,
            Tab {
                widget: MessagingUI::new(self.width, self.height - 1, status),
                src,
                style: TabStyle::Normal,
                switch,
                notifier,
            },
        );
    }

    fn find_serv_tab_idx(&self, serv_name_: &str) -> Option<usize> {
        for (tab_idx, tab) in self.tabs.iter().enumerate() {
            if let MsgSource::Serv { ref serv_name } = tab.src {
                if serv_name_ == serv_name {
                    return Some(tab_idx);
                }
            }
        }
        None
    }

    fn select_tab(&mut self, tab_idx: usize) {
        if tab_idx < self.active_idx {
            while tab_idx < self.active_idx {
                self.prev_tab_();
            }
        } else {
            while tab_idx > self.active_idx {
                self.next_tab_();
            }
        }
        self.tabs[self.active_idx].set_style(TabStyle::Normal);
    }

    fn next_tab_(&mut self) {
        if self.active_idx == self.tabs.len() - 1 {
            self.active_idx = 0;
            self.h_scroll = 0;
        } else {
            // either the next tab is visible, or we should scroll so that the
            // next tab becomes visible
            let next_active = self.active_idx + 1;
            loop {
                let (tab_left, tab_right) = self.rendered_tabs();
                if (next_active >= tab_left && next_active < tab_right)
                    || (next_active == tab_left && tab_left == tab_right)
                {
                    break;
                }
                self.h_scroll += self.tabs[tab_left].width() + 1;
            }
            self.active_idx = next_active;
        }
    }

    fn prev_tab_(&mut self) {
        if self.active_idx == 0 {
            let next_active = self.tabs.len() - 1;
            while self.active_idx != next_active {
                self.next_tab_();
            }
        } else {
            let next_active = self.active_idx - 1;
            loop {
                let (tab_left, tab_right) = self.rendered_tabs();
                if (next_active >= tab_left && next_active < tab_right)
                    || (next_active == tab_left && tab_left == tab_right)
                {
                    break;
                }
                self.h_scroll -= self.tabs[tab_left - 1].width() + 1;
            }
            if self.h_scroll < 0 {
                self.h_scroll = 0
            };
            self.active_idx = next_active;
        }
    }

    // right one is exclusive
    fn rendered_tabs(&self) -> (usize, usize) {
        let mut i = 0;

        {
            let mut skip = self.h_scroll;
            while skip > 0 && i < self.tabs.len() - 1 {
                skip -= self.tabs[i].width() + 1;
                i += 1;
            }
        }

        // drop tabs overflow on the right side
        let mut j = i;
        {
            // how much space left on screen
            let mut width_left = self.width;
            if self.draw_left_arrow() {
                width_left -= 2;
            }
            if self.draw_right_arrow() {
                width_left -= 2;
            }
            // drop any tabs that overflows from the screen
            for (tab_idx, tab) in (&self.tabs[i..]).iter().enumerate() {
                if tab.width() > width_left {
                    break;
                } else {
                    j += 1;
                    width_left -= tab.width();
                    if tab_idx != self.tabs.len() - i {
                        width_left -= 1;
                    }
                }
            }
        }

        debug_assert!(i < self.tabs.len());
        debug_assert!(j <= self.tabs.len());
        debug_assert!(i <= j);

        (i, j)
    }

    fn draw_left_arrow(&self) -> bool {
        self.h_scroll > 0
    }

    fn draw_right_arrow(&self) -> bool {
        let w1 = self.h_scroll + self.width;
        let w2 = {
            let mut w = if self.draw_left_arrow() { 2 } else { 0 };
            let last_tab_idx = self.tabs.len() - 1;
            for (tab_idx, tab) in self.tabs.iter().enumerate() {
                w += tab.width();
                if tab_idx != last_tab_idx {
                    w += 1;
                }
            }
            w
        };

        w2 > w1
    }
}

fn main() {}
