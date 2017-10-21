use futures::sync::mpsc as futures_mpsc;
use futures;
use futures::Future;
use serde_json;
use mio_more::channel::{channel, Sender, Receiver};
use slack_api;
use slack;
use reqwest::unstable::async as reqwest;
// use futures::sync::mpsc::SendError;
use tokio_core::reactor::Core;
use std::thread;
use websocket::async::futures::stream::SplitSink;
use websocket::async::futures::stream::SplitStream;
use futures::Stream;
use websocket::ClientBuilder;
use websocket;

pub enum Resp {
    ChannelList(Vec<slack_api::Channel>),
    ImList(Vec<slack_api::Im>),
    UserList(Vec<slack_api::User>),
    ChannelHistory(String, Vec<slack_api::Message>),
    SlackRTM(slack::Event),
    WS(websocket::OwnedMessage),
}

pub enum Req {
    ChannelList,
    UserList,
    ImList,
    ChannelHistory(String),
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
    pub http_thread: thread::JoinHandle<()>,
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
                Ok(Req::ImList) => {
                    let send = self.send.clone();
                    let token = self.token.clone();
                    let _ = thread::spawn(move || {
                        let client = slack_api::default_client();
                        let req = slack_api::im::ListRequest {
                            cursor: None,
                            limit: Some(5),
                        };
                        if let Ok(resp) = slack_api::im::list(&client, &token, &req) {
                            send.send(Resp::ImList(resp.ims.unwrap())).unwrap();
                        }
                    });
                }
                Ok(Req::UserList) => {
                    let send = self.send.clone();
                    let token = self.token.clone();
                    let _ = thread::spawn(move || {
                        let client = slack_api::default_client();
                        let req = slack_api::users::ListRequest::default();
                        if let Ok(resp) = slack_api::users::list(&client, &token, &req) {
                            send.send(Resp::UserList(resp.members.unwrap())).unwrap();
                        }
                    });
                },
                Ok(Req::ChannelHistory(chan_name)) => {
                    let send = self.send.clone();
                    let token = self.token.clone();
                    let _ = thread::spawn(move || {
                        let client = slack_api::default_client();
                        let resp = {
                            let req = slack_api::channels::HistoryRequest {
                                channel: &chan_name,
                                latest: None,
                                oldest: None,
                                inclusive: None,
                                count: Some(10),
                                unreads: None,
                            };
                            slack_api::channels::history(&client, &token, &req)
                        };
                        if let Ok(resp) = resp {
                            send.send(Resp::ChannelHistory(chan_name, resp.messages.unwrap())).unwrap();
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
    Pong(Vec<u8>),
    Close,
}

struct SlackWSConnHandle {
    send: futures_mpsc::Sender<SlackWSReq>,
    thr: thread::JoinHandle<()>,
}

fn spawn_ws_conn(token: String, send_resp: Sender<Resp>) -> SlackWSConnHandle {
    let (send_req, recv_req) = futures_mpsc::channel(10);
    let mut send_req_clone = send_req.clone();

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
                    .and_then(move |(sink, stream): (SplitSink<_>, SplitStream<_>)| {

                        let writer =
                            recv_req
                            .map_err(Error::Receiver)
                            .and_then(|e: SlackWSReq| {
                                match e {
                                    SlackWSReq::Close =>
                                        futures::future::err(Error::Close(())),
                                    SlackWSReq::Pong(data) =>
                                        futures::future::ok(websocket::OwnedMessage::Pong(data)),
                                }
                            })
                            .forward(sink);

                        let reader = stream.map_err(Error::WebSocket).for_each(
                            move |e: websocket::OwnedMessage| {
                                match e {
                                    websocket::OwnedMessage::Ping(data) => {
                                        send_req_clone.try_send(SlackWSReq::Pong(data)).unwrap();
                                    },
                                    websocket::OwnedMessage::Text(str) => {
                                        match serde_json::from_str(&str) {
                                            Ok(msg) => {
                                                send_resp.send(Resp::SlackRTM(msg)).unwrap();
                                            },
                                            Err(err) => {
                                                send_resp.send(Resp::WS(websocket::OwnedMessage::Text(str))).unwrap();
                                            },
                                        }
                                    },
                                    e => {
                                        send_resp.send(Resp::WS(e)).unwrap();
                                    }
                                }
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
        // Sender(err: SendError<websocket::OwnedMessage>) {
        //     description("sender error")
        //     display("Sender error")
        // }
        Close(err: ()) {
            description("close")
            display("Close") 
        }
    }
}
