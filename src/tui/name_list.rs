use termbox_simple::Termbox;
use term_input::Key;

use config::Colors;
use tui::termbox;
use tui::widget::WidgetRet;

pub struct NameList {
    names: Vec<Name>,
    scroll: i32,
    selection: i32,
    height: i32,
    width: i32,
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct Name {
    draw_str: String,
    presence: Option<String>,
    name: String,
}

impl Name {
    fn new(name: String, presence: Option<String>) -> Name {
        let draw_str = match presence {
            None =>
                name.clone(),
            Some(ref p) =>
                format!("[{}]{}", p, name),
        };
        Name { draw_str, name, presence }
    }

    fn draw(&self, tb: &mut Termbox, colors: &Colors, pos_x: i32, pos_y: i32, width: i32) {
        termbox::print_chars(
            tb,
            pos_x,
            pos_y,
            colors.user_msg,
            self.draw_str.chars().take(width as usize));
    }
}

impl NameList {
    pub fn new(width: i32, height: i32) -> NameList {
        NameList {
            names: vec![],
            scroll: 0,
            selection: 0,
            height: height,
            width: width,
        }
    }

    pub fn width(&self) -> i32 {
        self.width
    }

    pub fn resize(&mut self, width: i32, height: i32) {
        self.width = width;
        self.height = height;
    }

    pub fn draw(&self, tb: &mut Termbox, colors: &Colors, pos_x: i32, pos_y: i32) {
        let range_begin = self.scroll as usize;
        let range_end = ::std::cmp::min((self.scroll + self.height) as usize, self.names.len());
        let names_range = &self.names[range_begin .. range_end];
        for (name_idx, name) in names_range.iter().enumerate() {
            name.draw(tb, colors, pos_x, pos_y + name_idx as i32, self.width);
        }
    }

    pub fn keypressed(&mut self, key: Key) -> WidgetRet {
        // TODO
        unimplemented!()
    }

    pub fn join(&mut self, nick: String) {
        let name = Name::new(nick, None);
        if let Err(idx) = self.names.binary_search(&name) {
            self.names.insert(idx, name);
        }
    }

    pub fn part(&mut self, nick: &str) {
        if let Ok(idx) = self.names.binary_search_by(|s| s.name.as_str().cmp(nick)) {
            self.names.remove(idx);
        }
    }

    pub fn set_presence(&mut self, nick: &str, presence: &str) {
        self.part(nick);
        let name = Name::new(nick.to_owned(), Some(presence.to_owned()));
        if let Err(idx) = self.names.binary_search(&name) {
            self.names.insert(idx, name);
        }
    }
}
