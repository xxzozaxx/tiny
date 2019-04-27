#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    /// Termbox fg.
    pub fg: u16,

    /// Termbox bg.
    pub bg: u16,
}

#[derive(Debug)]
pub struct Colors {
    pub nick: Vec<u8>,
    pub clear: Color,
    pub user_msg: Color,
    pub err_msg: Color,
    pub topic: Color,
    pub cursor: Color,
    pub join: Color,
    pub part: Color,
    pub nick_change: Color,
    pub faded: Color,
    pub exit_dialogue: Color,
    pub highlight: Color,
    pub completion: Color,
    pub timestamp: Color,
    pub tab_active: Color,
    pub tab_normal: Color,
    pub tab_new_msg: Color,
    pub tab_highlight: Color,
}
