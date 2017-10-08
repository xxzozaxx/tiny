use futures::Map;
use futures::sink::SendAll;
use futures::sink::SinkFromErr;
use futures::stream::Forward;
use futures::sync::mpsc::channel;
use futures::sync::mpsc::Receiver;
use futures::sync::mpsc::Sender;
use futures::sync::mpsc::SendError;
use futures::{Future, Stream, Sink};
use futures;
use reqwest::unstable::async as reqwest;
use serde;
use serde_json;
use slack_api;
use std::boxed::FnBox;
use std::io::Write;
use std::sync::Arc;
use std::sync::Mutex;
use tokio_core::reactor::Core;
use tui::messaging::Timestamp;
use tui::MsgTarget;
use tui;
use websocket::async::futures::stream::SplitSink;
use websocket::async::futures::stream::SplitStream;
use websocket::client::async::ClientNew;
use websocket::ClientBuilder;
use websocket;

// pub fn main(recv: Receiver<String>, send: Sender<websocket::OwnedMessage>) -> Box<FnBox() -> () + Send> {
pub fn main(tui: Arc<Mutex<tui::TUI>>, recv: Receiver<String>) -> Box<FnBox() -> () + Send> {
    Box::new(move || {
        tui.lock().unwrap().new_chan_tab("slack", "slack");

        let tui1 = tui.clone();
        let tui2 = tui.clone();

        let mut core = Core::new().unwrap();
        let handle = core.handle();

        let api_tok = ::std::env::var("SLACK_API_TOK").unwrap();
        let http_client = reqwest::Client::new(&handle);

        let f = slack_api::rtm::start_async(&http_client, &api_tok, &Default::default())
            .map_err(|e| Error::Receiver(()))
            .and_then(move |r| {
                let url = r.url.unwrap();
                // let url = Url::parse(&url).unwrap();
                ClientBuilder::new(&url)
                    .unwrap()
                    .async_connect(None, &handle)
                    .map_err(Error::WebSocket)
                    .map(|(duplex, _)| duplex.split())
                    .and_then(move |(sink, stream): (SplitSink<_>, SplitStream<_>)| {

                        let writer = recv.map_err(Error::Receiver).for_each(
                            move |e: String| {
                                tui1.lock().unwrap().add_privmsg(
                                    "client",
                                    &format!("msg: {}", e),
                                    Timestamp::now(),
                                    &MsgTarget::AllTabs);
                                tui1.lock().unwrap().draw();
                                if e == "exit" {
                                    return futures::future::err(Error::Receiver(()))
                                } else {
                                    futures::future::ok(())
                                }
                            });

                            // .send_all(recv.map(websocket::OwnedMessage::Text).map_err(Error::Receiver));

                        let reader = stream.map_err(Error::WebSocket).for_each(
                            move |e: websocket::OwnedMessage| {
                                tui2.lock().unwrap().add_privmsg(
                                    "slack server",
                                    &format!("{:?}", e),
                                    Timestamp::now(),
                                    &MsgTarget::AllTabs,
                                );
                                tui2.lock().unwrap().draw();
                                futures::future::ok(())
                            },
                        );

                        reader.join(writer)
                    })
            });

        let _ = core.run(f);
    })
}

quick_error! {
    #[derive(Debug)]
    pub enum Error {
        WebSocket(err: websocket::WebSocketError) {
            from()
            description("websocket error")
            display("WebSocket error: {}", err)
            cause(err)
        }
        Serde(err: serde_json::error::Error) {
            from()
            description("serde_json error")
            display("Serde JSON error: {}", err)
            cause(err)
        }
        Receiver(err: ()) {
            description("receiver error")
            display("Receiver error")
        }
        Sender(err: SendError<websocket::OwnedMessage>) {
            description("sender error")
            display("Sender error")
        }
    }
}
