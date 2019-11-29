// A program that initializes the TUI and adds lines in the file (given as first argument) to it,
// with a draw() call after every line added.
//
// After adding all lines the program just quits.
//
// Useful for benchmarking TUI::draw().

use libtiny_tui::{Colors, TUI};
use libtiny_ui::*;
use std::fs::File;
use std::io::{BufRead, BufReader};
use tokio::runtime::Runtime;
use tokio::task::LocalSet;

fn main() {
    let args = std::env::args().collect::<Vec<_>>();
    let file_path = &args[1];
    let file = File::open(file_path).unwrap();
    let file_buffered = BufReader::new(file);
    let lines = file_buffered.lines().map(Result::unwrap).collect();

    let mut executor = Runtime::new().unwrap();
    let local_set = LocalSet::new();

    let (tui, _) = TUI::run(Colors::default(), &local_set);

    tui.new_server_tab("test");
    tui.draw();

    local_set.block_on(&mut executor, bench_task(tui, lines));
}

async fn bench_task(tui: TUI, lines: Vec<String>) {
    let msg_target = MsgTarget::Server { serv: "test" };
    let time = time::now();

    for line in &lines {
        tui.add_privmsg("server", line, time, &msg_target, false, false);
        tui.draw();
    }
}
