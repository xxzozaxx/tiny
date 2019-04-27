//! Defines the trait for UIs and helper types.

use time::Tm;

/// Target of a message to be shown on a UI.
pub enum MsgTarget<'a> {
    /// Show it in a server tab.
    Server { serv_name: &'a str },

    /// Show it in a channel tab.
    Chan {
        serv_name: &'a str,
        chan_name: &'a str,
    },

    /// Show it in a privmsg tab.
    User { serv_name: &'a str, nick: &'a str },

    /// Show it in all tabs of a server.
    AllServTabs { serv_name: &'a str },

    /// Show it in all server tabs that have the user. (i.e. channels, privmsg tabs)
    AllUserTabs { serv_name: &'a str, nick: &'a str },

    /// Show it in the currently active tab.
    CurrentTab,
}

/// Source of a message from the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MsgSource {
    /// Message sent in a server tab.
    Serv { serv_name: String },

    /// Message sent in a channel tab.
    Chan {
        serv_name: String,
        chan_name: String,
    },

    /// Message sent in a privmsg tab.
    User { serv_name: String, nick: String },
}

impl MsgSource {
    pub fn serv_name(&self) -> &str {
        match *self {
            MsgSource::Serv { ref serv_name }
            | MsgSource::Chan { ref serv_name, .. }
            | MsgSource::User { ref serv_name, .. } => serv_name,
        }
    }

    pub fn to_target(&self) -> MsgTarget {
        match *self {
            MsgSource::Serv { ref serv_name } => MsgTarget::Server { serv_name },
            MsgSource::Chan {
                ref serv_name,
                ref chan_name,
            } => MsgTarget::Chan {
                serv_name,
                chan_name,
            },
            MsgSource::User {
                ref serv_name,
                ref nick,
            } => MsgTarget::User { serv_name, nick },
        }
    }

    pub fn visible_name(&self) -> &str {
        match *self {
            MsgSource::Serv { ref serv_name, .. } => serv_name,
            MsgSource::Chan { ref chan_name, .. } => chan_name,
            MsgSource::User { ref nick, .. } => nick,
        }
    }
}

/// A UI event.
pub enum UIEv {
    /// Quit
    Abort,

    /// An input from the user
    Input { msg: Vec<char>, from: MsgSource },

    /// A pasted string. TODO: This is kinda hacky, maybe remove?
    Lines { lines: Vec<String>, from: MsgSource },
}

pub trait UI {
    type InputEvent;

    fn new_server_tab(&mut self, serv_name: &str);
    fn close_server_tab(&mut self, serv_name: &str);

    fn new_chan_tab(&self, serv_name: &str, chan_name: &str);
    fn close_chan_tab(&self, serv_name: &str, chan_name: &str);

    fn new_user_tab(&self, serv_name: &str, nick: &str);
    fn close_user_tab(&self, serv_name: &str, nick: &str);

    fn handle_input_event(&self, ev: Self::InputEvent);

    fn resize(&self);

    /// Implements the `/switch <name>` command.
    fn switch(&self, string: &str);

    /// An error message coming from the library, probably because of a command error etc.
    fn add_client_err_msg(&mut self, msg: &str, target: &MsgTarget);

    /// A notify message coming from the library, usually shows a response of a command.
    /// e.g. "Notifications enabled".
    fn add_client_notify_msg(&mut self, msg: &str, target: &MsgTarget);

    /// A message from the library, usually just to indidate progress, e.g. "Connecting...".
    fn add_client_msg(&mut self, msg: &str, target: &MsgTarget);

    /// privmsg is a message coming from a remote server or the user. Show with sender's nick/name
    /// and receive time.
    fn add_privmsg(
        &mut self,
        sender: &str,
        msg: &str,
        ts: Tm,
        target: &MsgTarget,
        ctcp_action: bool,
    );

    /// Similar to `add_privmsg`, except the whole message is highlighted.
    fn add_privmsg_highlight(
        &mut self,
        sender: &str,
        msg: &str,
        ts: Tm,
        target: &MsgTarget,
        ctcp_action: bool,
    );

    /// A message without any explicit sender info. Useful for e.g. in server and debug log tabs.
    fn add_msg(&mut self, msg: &str, ts: Tm, target: &MsgTarget);

    /// Error messages related with the protocol - e.g. "can't join channel", "nickname is in use"
    /// etc.
    fn add_err_msg(&mut self, msg: &str, ts: Tm, target: &MsgTarget);

    /// Implements the `/topic` command.
    fn show_topic(&mut self, title: &str, ts: Tm, target: &MsgTarget);

    // TODO: These should take a server and channel name?
    /// Clear nick list.
    fn clear_nicks(&mut self, target: &MsgTarget);

    fn add_nick(&mut self, nick: &str, ts: Option<Tm>, target: &MsgTarget);
    fn remove_nick(&mut self, nick: &str, ts: Option<Tm>, target: &MsgTarget);
    fn rename_nick(&mut self, old_nick: &str, new_nick: &str, ts: Tm, target: &MsgTarget);

    /// Set our nick in a server.
    fn set_nick(&mut self, serv_name: &str, new_nick: &str);

    /// Clear the chat buffer.
    fn clear(&mut self, target: &MsgTarget);

    /// Implements `/ignore` command.
    fn toggle_ignore(&mut self, target: &MsgTarget);

    // TODO: Why is this needed? Remove this.
    fn does_user_tab_exist(&self, serv_name_: &str, nick_: &str) -> bool;
}
