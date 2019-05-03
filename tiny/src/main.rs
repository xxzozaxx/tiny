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
use messaging::Timestamp;
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

    fn new_chan_tab(&mut self, serv_name: &str, chan_name: &str) {
        self.new_chan_tab_(serv_name, chan_name);
    }

    fn close_chan_tab(&mut self, serv_name: &str, chan_name: &str) {
        if let Some(tab_idx) = self.find_chan_tab_idx(serv_name, chan_name) {
            self.tabs.remove(tab_idx);
            if self.active_idx == tab_idx {
                self.select_tab(if tab_idx == 0 { 0 } else { tab_idx - 1 });
            }
        }
    }

    fn new_user_tab(&mut self, serv_name: &str, nick: &str) {
        self.new_user_tab_(serv_name, nick);
    }

    fn close_user_tab(&mut self, serv_name: &str, nick: &str) {
        if let Some(tab_idx) = self.find_user_tab_idx(serv_name, nick) {
            self.tabs.remove(tab_idx);
            if self.active_idx == tab_idx {
                self.select_tab(if tab_idx == 0 { 0 } else { tab_idx - 1 });
            }
        }
    }

    fn handle_input_event(&self, ev: Self::InputEvent) {
        unimplemented!()
    }

    fn resize(&mut self) {
        self.tb.resize();
        self.tb.clear();

        self.width = self.tb.width();
        self.height = self.tb.height();

        for tab in &mut self.tabs {
            tab.widget.resize(self.width, self.height - 1);
        }
        // scroll the tab bar so that currently active tab is still visible
        let (mut tab_left, mut tab_right) = self.rendered_tabs();
        if tab_left == tab_right {
            // nothing to show
            return;
        }
        while self.active_idx < tab_left || self.active_idx >= tab_right {
            if self.active_idx >= tab_right {
                // scroll right
                self.h_scroll += self.tabs[tab_left].width() + 1;
            } else if self.active_idx < tab_left {
                // scroll left
                self.h_scroll -= self.tabs[tab_left - 1].width() + 1;
            }
            let (tab_left_, tab_right_) = self.rendered_tabs();
            tab_left = tab_left_;
            tab_right = tab_right_;
        }
        // the selected tab is visible. scroll to the left as much as possible
        // to make more tabs visible.
        let mut num_visible = tab_right - tab_left;
        loop {
            if tab_left == 0 {
                break;
            }
            // save current scroll value
            let scroll_orig = self.h_scroll;
            // scoll to the left
            self.h_scroll -= self.tabs[tab_left - 1].width() + 1;
            // get new bounds
            let (tab_left_, tab_right_) = self.rendered_tabs();
            // commit if these two conditions hold
            let num_visible_ = tab_right_ - tab_left_;
            let more_tabs_visible = num_visible_ > num_visible;
            let selected_tab_visible = self.active_idx >= tab_left_ && self.active_idx < tab_right_;
            if !(more_tabs_visible && selected_tab_visible) {
                // revert scroll value and abort
                self.h_scroll = scroll_orig;
                break;
            }
            // otherwise commit
            tab_left = tab_left_;
            num_visible = num_visible_;
        }
    }

    fn switch(&mut self, string: &str) {
        let mut next_idx = self.active_idx;
        for (tab_idx, tab) in self.tabs.iter().enumerate() {
            match tab.src {
                MsgSource::Serv { ref serv_name } => {
                    if serv_name.contains(string) {
                        next_idx = tab_idx;
                        break;
                    }
                }
                MsgSource::Chan { ref chan_name, .. } => {
                    if chan_name.contains(string) {
                        next_idx = tab_idx;
                        break;
                    }
                }
                MsgSource::User { ref nick, .. } => {
                    if nick.contains(string) {
                        next_idx = tab_idx;
                        break;
                    }
                }
            }
        }
        if next_idx != self.active_idx {
            self.select_tab(next_idx);
        }
    }

    fn add_client_err_msg(&mut self, msg: &str, target: &MsgTarget) {
        self.apply_to_target(target, &|tab: &mut Tab, _| {
            tab.widget.add_client_err_msg(msg);
        });
    }

    fn add_client_notify_msg(&mut self, msg: &str, target: &MsgTarget) {
        self.apply_to_target(target, &|tab: &mut Tab, _| {
            tab.widget.add_client_notify_msg(msg);
        });
    }

    fn add_client_msg(&mut self, msg: &str, target: &MsgTarget) {
        self.apply_to_target(target, &|tab: &mut Tab, _| {
            tab.widget.add_client_msg(msg);
        });
    }

    fn add_privmsg(
        &mut self,
        sender: &str,
        msg: &str,
        ts: Tm,
        target: &MsgTarget,
        ctcp_action: bool,
    ) {
        self.apply_to_target(target, &|tab: &mut Tab, _| {
            tab.widget
                .add_privmsg(sender, msg, Timestamp::from(ts), false, ctcp_action);
            let nick = tab.widget.get_nick();
            if let Some(nick_) = nick {
                tab.notifier
                    .notify_privmsg(sender, msg, target, nick_, false);
            }
        });
    }

    fn add_privmsg_highlight(
        &mut self,
        sender: &str,
        msg: &str,
        ts: Tm,
        target: &MsgTarget,
        ctcp_action: bool,
    ) {
        self.apply_to_target(target, &|tab: &mut Tab, _| {
            tab.widget
                .add_privmsg(sender, msg, Timestamp::from(ts), true, ctcp_action);
            let nick = tab.widget.get_nick();
            if let Some(nick_) = nick {
                tab.notifier
                    .notify_privmsg(sender, msg, target, nick_, true);
            }
        });
    }

    fn add_msg(&mut self, msg: &str, ts: Tm, target: &MsgTarget) {
        self.apply_to_target(target, &|tab: &mut Tab, _| {
            tab.widget.add_msg(msg, Timestamp::from(ts));
        });
    }

    fn add_err_msg(&mut self, msg: &str, ts: Tm, target: &MsgTarget) {
        self.apply_to_target(target, &|tab: &mut Tab, _| {
            tab.widget.add_err_msg(msg, Timestamp::from(ts));
        });
    }

    fn show_topic(&mut self, title: &str, ts: Tm, target: &MsgTarget) {
        self.apply_to_target(target, &|tab: &mut Tab, _| {
            tab.widget.show_topic(title, Timestamp::from(ts));
        });
    }

    fn clear_nicks(&mut self, target: &MsgTarget) {
        self.apply_to_target(target, &|tab: &mut Tab, _| {
            tab.widget.clear_nicks();
        });
    }

    fn add_nick(&mut self, nick: &str, ts: Option<Tm>, target: &MsgTarget) {
        self.apply_to_target(target, &|tab: &mut Tab, _| {
            tab.widget.join(nick, ts.map(Timestamp::from));
        });
    }

    fn remove_nick(&mut self, nick: &str, ts: Option<Tm>, target: &MsgTarget) {
        self.apply_to_target(target, &|tab: &mut Tab, _| {
            tab.widget.part(nick, ts.map(Timestamp::from));
        });
    }

    fn rename_nick(&mut self, old_nick: &str, new_nick: &str, ts: Tm, target: &MsgTarget) {
        self.apply_to_target(target, &|tab: &mut Tab, _| {
            tab.widget.nick(old_nick, new_nick, Timestamp::from(ts));
            tab.update_source(&|src: &mut MsgSource| {
                if let MsgSource::User { ref mut nick, .. } = *src {
                    nick.clear();
                    nick.push_str(new_nick);
                }
            });
        });
    }

    fn set_nick(&mut self, serv_name: &str, new_nick: &str) {
        let target = MsgTarget::AllServTabs { serv_name };
        self.apply_to_target(&target, &|tab: &mut Tab, _| {
            tab.widget.set_nick(new_nick.to_owned())
        });
    }

    fn clear(&mut self, target: &MsgTarget) {
        self.apply_to_target(target, &|tab: &mut Tab, _| tab.widget.clear());
    }

    fn toggle_ignore(&mut self, target: &MsgTarget) {
        if let MsgTarget::AllServTabs { serv_name } = *target {
            let mut status_val: bool = false;
            for tab in &self.tabs {
                if let MsgSource::Serv {
                    serv_name: ref serv_name_,
                } = tab.src
                {
                    if serv_name == serv_name_ {
                        status_val = tab.widget.get_ignore_state();
                        break;
                    }
                }
            }
            self.apply_to_target(target, &|tab: &mut Tab, _| {
                tab.widget.set_or_toggle_ignore(Some(!status_val));
            });
        } else {
            self.apply_to_target(target, &|tab: &mut Tab, _| {
                tab.widget.set_or_toggle_ignore(None);
            });
        }
        // Changing tab names (adding "[i]" suffix) may make the tab currently
        // selected overflow from the screen. Easiest (although not most
        // efficient) way to fix this is `resize()`.
        self.resize();
    }

    fn does_user_tab_exist(&self, serv_name_: &str, nick_: &str) -> bool {
        for tab in &self.tabs {
            if let MsgSource::User {
                ref serv_name,
                ref nick,
            } = tab.src
            {
                if serv_name_ == serv_name && nick_ == nick {
                    return true;
                }
            }
        }
        false
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

    fn new_chan_tab_(&mut self, serv_name: &str, chan_name: &str) -> Option<usize> {
        match self.find_chan_tab_idx(serv_name, chan_name) {
            None => match self.find_last_serv_tab_idx(serv_name) {
                None => {
                    self.new_server_tab(serv_name);
                    self.new_chan_tab_(serv_name, chan_name)
                }
                Some(serv_tab_idx) => {
                    let mut status_val: bool = true;
                    let mut notifier = Notifier::Mentions;
                    for tab in &self.tabs {
                        if let MsgSource::Serv {
                            serv_name: ref serv_name_,
                        } = tab.src
                        {
                            if serv_name == serv_name_ {
                                status_val = tab.widget.get_ignore_state();
                                notifier = tab.notifier;
                                break;
                            }
                        }
                    }
                    let tab_idx = serv_tab_idx + 1;
                    self.new_tab(
                        tab_idx,
                        MsgSource::Chan {
                            serv_name: serv_name.to_owned(),
                            chan_name: chan_name.to_owned(),
                        },
                        status_val,
                        notifier,
                    );
                    if self.active_idx >= tab_idx {
                        self.next_tab();
                    }
                    if let Some(nick) = self.tabs[serv_tab_idx].widget.get_nick().map(str::to_owned)
                    {
                        self.tabs[tab_idx].widget.set_nick(nick);
                    }
                    Some(tab_idx)
                }
            },
            Some(_) => None,
        }
    }

    fn new_user_tab_(&mut self, serv_name: &str, nick: &str) -> Option<usize> {
        match self.find_user_tab_idx(serv_name, nick) {
            None => match self.find_last_serv_tab_idx(serv_name) {
                None => {
                    self.new_server_tab(serv_name);
                    self.new_user_tab_(serv_name, nick)
                }
                Some(tab_idx) => {
                    self.new_tab(
                        tab_idx + 1,
                        MsgSource::User {
                            serv_name: serv_name.to_owned(),
                            nick: nick.to_owned(),
                        },
                        true,
                        Notifier::Messages,
                    );
                    if let Some(nick) = self.tabs[tab_idx].widget.get_nick().map(str::to_owned) {
                        self.tabs[tab_idx + 1].widget.set_nick(nick);
                    }
                    self.tabs[tab_idx + 1].widget.join(nick, None);
                    Some(tab_idx + 1)
                }
            },
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

    /// Index of the last tab with the given server name.
    fn find_last_serv_tab_idx(&self, serv_name: &str) -> Option<usize> {
        for (tab_idx, tab) in self.tabs.iter().enumerate().rev() {
            if tab.src.serv_name() == serv_name {
                return Some(tab_idx);
            }
        }
        None
    }

    fn find_chan_tab_idx(&self, serv_name_: &str, chan_name_: &str) -> Option<usize> {
        for (tab_idx, tab) in self.tabs.iter().enumerate() {
            if let MsgSource::Chan {
                ref serv_name,
                ref chan_name,
            } = tab.src
            {
                if serv_name_ == serv_name && chan_name_ == chan_name {
                    return Some(tab_idx);
                }
            }
        }
        None
    }

    fn find_user_tab_idx(&self, serv_name_: &str, nick_: &str) -> Option<usize> {
        for (tab_idx, tab) in self.tabs.iter().enumerate() {
            if let MsgSource::User {
                ref serv_name,
                ref nick,
            } = tab.src
            {
                if serv_name_ == serv_name && nick_ == nick {
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

    fn next_tab(&mut self) {
        self.next_tab_();
        self.tabs[self.active_idx].set_style(TabStyle::Normal);
    }

    fn apply_to_target<F>(&mut self, target: &MsgTarget, f: &F)
    where
        F: Fn(&mut Tab, bool),
    {
        // Creating a vector just to make borrow checker happy. Borrow checker
        // sucks once more. Here it sucks 2x, I can't even create a Vec<&mut Tab>,
        // I need a Vec<usize>.
        //
        // (I could use an array on stack but whatever)
        let mut target_idxs: Vec<usize> = Vec::with_capacity(1);

        match *target {
            MsgTarget::Server { serv_name } => {
                for (tab_idx, tab) in self.tabs.iter().enumerate() {
                    if let MsgSource::Serv {
                        serv_name: ref serv_name_,
                    } = tab.src
                    {
                        if serv_name == serv_name_ {
                            target_idxs.push(tab_idx);
                            break;
                        }
                    }
                }
            }

            MsgTarget::Chan {
                serv_name,
                chan_name,
            } => {
                for (tab_idx, tab) in self.tabs.iter().enumerate() {
                    if let MsgSource::Chan {
                        serv_name: ref serv_name_,
                        chan_name: ref chan_name_,
                    } = tab.src
                    {
                        if serv_name == serv_name_ && chan_name == chan_name_ {
                            target_idxs.push(tab_idx);
                            break;
                        }
                    }
                }
            }

            MsgTarget::User { serv_name, nick } => {
                for (tab_idx, tab) in self.tabs.iter().enumerate() {
                    if let MsgSource::User {
                        serv_name: ref serv_name_,
                        nick: ref nick_,
                    } = tab.src
                    {
                        if serv_name == serv_name_ && nick == nick_ {
                            target_idxs.push(tab_idx);
                            break;
                        }
                    }
                }
            }

            MsgTarget::AllServTabs { serv_name } => {
                for (tab_idx, tab) in self.tabs.iter().enumerate() {
                    if tab.src.serv_name() == serv_name {
                        target_idxs.push(tab_idx);
                    }
                }
            }

            MsgTarget::AllUserTabs { serv_name, nick } => {
                for (tab_idx, tab) in self.tabs.iter().enumerate() {
                    match tab.src {
                        MsgSource::Serv { .. } => {}
                        MsgSource::Chan {
                            serv_name: ref serv_name_,
                            ..
                        } => {
                            if serv_name_ == serv_name && tab.widget.has_nick(nick) {
                                target_idxs.push(tab_idx);
                            }
                        }
                        MsgSource::User {
                            serv_name: ref serv_name_,
                            nick: ref nick_,
                        } => {
                            if serv_name_ == serv_name && nick_ == nick {
                                target_idxs.push(tab_idx);
                            }
                        }
                    }
                }
            }

            MsgTarget::CurrentTab => {
                target_idxs.push(self.active_idx);
            }
        }

        // Create server/chan/user tab when necessary
        if target_idxs.is_empty() {
            if let Some(idx) = self.maybe_create_tab(target) {
                target_idxs.push(idx);
            }
        }

        for tab_idx in target_idxs {
            f(&mut self.tabs[tab_idx], self.active_idx == tab_idx);
        }
    }

    fn maybe_create_tab(&mut self, target: &MsgTarget) -> Option<usize> {
        match *target {
            MsgTarget::Server { serv_name } | MsgTarget::AllServTabs { serv_name } => {
                self.new_server_tab_(serv_name)
            }

            MsgTarget::Chan {
                serv_name,
                chan_name,
            } => self.new_chan_tab_(serv_name, chan_name),

            MsgTarget::User { serv_name, nick } => self.new_user_tab_(serv_name, nick),

            _ => None,
        }
    }
}

fn main() {}
