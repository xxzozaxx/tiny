use futures::sync::mpsc as futures_mpsc;
use futures;
use futures::Future;
use serde_json;
use mio_more::channel::{channel, Sender, Receiver};
use slack_api;
use reqwest::unstable::async as reqwest;
use tokio_core::reactor::Core;
use std::thread;
use websocket::async::futures::stream::SplitSink;
use websocket::async::futures::stream::SplitStream;
use futures::Stream;
use websocket::ClientBuilder;
use websocket;

pub enum Resp {
    ChannelList(Vec<slack_api::Channel>),
    WS(websocket::OwnedMessage),
}

pub enum Req {
    ChannelList,
    Close,
}

struct SlackHTTPConn {
    send: Sender<Resp>,
    recv: Receiver<Req>,
    token: String,
    ws_conn_handle: SlackWSConnHandle,
}

pub struct SlackConnHandle {
    pub send: Sender<Req>,
    pub recv: Receiver<Resp>,
    // pub ws_send: 
    pub http_thread: thread::JoinHandle<()>,
    // pub ws_thread: thread::JoinHandle<()>,
}

pub fn spawn(token: String) -> SlackConnHandle {
    let (send_req, recv_req) = channel();
    let (send_resp, recv_resp) = channel();

    let conn = SlackHTTPConn {
        send: send_resp.clone(),
        recv: recv_req,
        token: token.clone(),
        ws_conn_handle: spawn_ws_conn(token, send_resp),
    };

    let thr = thread::spawn(move || {
        conn.run();
    });

    SlackConnHandle {
        send: send_req,
        recv: recv_resp,
        http_thread: thr,
    }
}

impl SlackHTTPConn {
    pub fn run(mut self) {
        loop {
            match self.recv.recv() {
                Ok(Req::Close) => {
                    break;
                }
                Ok(Req::ChannelList) => {
                    let send = self.send.clone();
                    let token = self.token.clone();
                    let _ = thread::spawn(move || {
                        let client = slack_api::default_client();
                        let req = slack_api::channels::ListRequest::default();
                        if let Ok(resp) = slack_api::channels::list(&client, &token, &req) {
                            send.send(Resp::ChannelList(resp.channels.unwrap())).unwrap();
                        }
                    });
                }
                Err(_) => {
                    break;
                }
            }
        }

        self.ws_conn_handle.send.try_send(SlackWSReq::Close).unwrap();
        self.ws_conn_handle.thr.join().unwrap();
    }
}

enum SlackWSReq {
    Close,
}

struct SlackWSConnHandle {
    send: futures_mpsc::Sender<SlackWSReq>,
    thr: thread::JoinHandle<()>,
}

fn spawn_ws_conn(token: String, send_resp: Sender<Resp>) -> SlackWSConnHandle {
    let (send_req, recv_req) = futures_mpsc::channel(10);

    let thr = thread::spawn(move || {

        let mut core = Core::new().unwrap();
        let handle = core.handle();

        let http_client = reqwest::Client::new(&handle);

        let f = slack_api::rtm::start_async(&http_client, &token, &Default::default())
            .map_err(|_: slack_api::rtm::StartError<_>| Error::Receiver(()))
            .and_then(move |r| {
                let url = r.url.unwrap();
                // let url = Url::parse(&url).unwrap();
                ClientBuilder::new(&url)
                    .unwrap()
                    .async_connect(None, &handle)
                    .map_err(Error::WebSocket)
                    .map(|(duplex, _)| duplex.split())
                    .and_then(move |(_sink, stream): (SplitSink<_>, SplitStream<_>)| {

                        let writer = recv_req.map_err(Error::Receiver).for_each(
                            move |e: SlackWSReq| {
                                match e {
                                    SlackWSReq::Close => 
                                        futures::future::err(Error::Receiver(()))
                                }
                            });

                        let reader = stream.map_err(Error::WebSocket).for_each(
                            move |e: websocket::OwnedMessage| {
                                send_resp.send(Resp::WS(e)).unwrap();
                                futures::future::ok(())
                            },
                            );

                        reader.join(writer)
                    })
            });

        let _ = core.run(f);
    });

    SlackWSConnHandle {
        send: send_req,
        thr: thr,
    }
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
        Sender(err: futures_mpsc::SendError<websocket::OwnedMessage>) {
            description("sender error")
            display("Sender error")
        }
    }
}
