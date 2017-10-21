use termbox_simple::Termbox;
use term_input::Key;

use config::Colors;
use tui::termbox;
use tui::widget::WidgetRet;

pub struct NameList {
    names: Vec<String>,
    scroll: i32,
    selection: i32,
    height: i32,
    width: i32,
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
            termbox::print_chars(
                tb,
                pos_x,
                pos_y + name_idx as i32,
                colors.user_msg,
                name.chars().take(self.width as usize));
        }
    }

    pub fn keypressed(&mut self, key: Key) -> WidgetRet {
        // TODO
        unimplemented!()
    }

    pub fn join(&mut self, nick: String) {
        if let Err(idx) = self.names.binary_search(&nick) {
            self.names.insert(idx, nick);
        }
    }

    pub fn part(&mut self, nick: &str) {
        if let Ok(idx) = self.names.binary_search_by(|s| s.as_str().cmp(nick)) {
            self.names.remove(idx);
        }
    }
}
