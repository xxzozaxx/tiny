extern crate tiny;

use tiny::tui::paste_lines;

fn main() {
    println!("{:?}", paste_lines("\nfoo\nbar"));
}
