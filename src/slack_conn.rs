use mio::Events;
use mio::Poll;
use mio::PollOpt;
use mio::Ready;
use mio::Token;
use mio::unix::EventedFd;
use native_tls::TlsStream;
use native_tls::{TlsConnector, HandshakeError as TlsHandshakeError};
use slack;
use slack_api::requests::Client as SlackHTTPClient;
use slack_api;
use std::collections::HashMap;
use std::net::{TcpStream, SocketAddr, ToSocketAddrs};
use std::os::unix::io::AsRawFd;
use std;
use tungstenite::protocol::WebSocket;
use tungstenite;
use url::Url;

use futures::future::Executor;
use futures::future::Future;
use reqwest::unstable::async as reqwest;
use tokio_core::io::{read_to_end, write_all};
use tokio_core::reactor::Core;

use std::sync::{Arc, Mutex};


pub struct SlackConn {
    api_tok: String,

    /// HTTP client to be used when calling slack HTTP API
    http_client: SlackHTTPClient,

    /// Websocket connection to the slack server
    ws: WebSocket<TlsStream<TcpStream>>,

    /// Channel id -> channel name map
    chans: HashMap<String, String>,

    /// User id -> user name map
    users: HashMap<String, String>,
}

impl SlackConn {
    pub fn new() {

        // Create the event loop that will drive this server.
        let mut core = Core::new().unwrap();
        let handle = core.handle();

        let api_tok = ::std::env::var("SLACK_API_TOK").unwrap();

        let mut client = reqwest::Client::new(&handle);

        let chans_future = slack_api::channels::list_async(
            &client,
            &api_tok,
            &slack_api::channels::ListRequest {
                exclude_archived: Some(true),
                exclude_members: Some(false),
            })
            .map_err(|e| println!("error: {:?}", e))
            .map(|r| println!("got chan list response"));

        let users_future = slack_api::users::list_async(
            &client,
            &api_tok,
            &slack_api::users::ListRequest {
                presence: Some(true),
            })
            .map_err(|e| println!("error: {:?}", e))
            .map(|r| println!("got user list response"));

        handle.execute(chans_future);
        println!("spawned chan future");
        handle.execute(users_future);
        println!("spawned users future");

        println!("asking for RTM url");
        let poll = Arc::new(Mutex::new(Poll::new().unwrap()));

        let rtm_future = slack_api::rtm::start_async(
            &client,
            &api_tok,
            &Default::default())
            .map_err(|e| println!("error: {:?}", e))
            .map(|r| {
                let url = r.url.unwrap();
                let url = Url::parse(&url).unwrap();
                let domain = url.host_str().unwrap();

                println!("url: {:?}, domain: {:?}", url, domain);

                let addrs = url.to_socket_addrs().unwrap();
                let stream = connect_to_some(addrs, &url);

                let ws: WebSocket<TlsStream<TcpStream>> =
                    tungstenite::client(tungstenite::handshake::client::Request::from(url.clone()), stream).unwrap().0;

                //poll.lock().unwrap().register(
                //    &EventedFd(&ws.get_ref().get_ref().as_raw_fd()),
                //    Token(ws.get_ref().get_ref().as_raw_fd() as usize),
                //    Ready::readable() | Ready::writable(),
                //    PollOpt::edge()).unwrap();
                ws
            });

        println!("waiting for RTM response");
        match core.run(rtm_future) {
            Err(()) => {},
            Ok(_) => {
                println!("got response");
            }
        }

        // println!("creating event loop");

        // let poll = Poll::new().unwrap();
        // tungstenite::client::client(


        // {
        //     let client = ::reqwest::Client::new().unwrap();
        //     let users = slack_api::users::list(
        //         &client,
        //         &api_tok,
        //         &slack_api::users::ListRequest {
        //             presence: Some(true),
        //         });
        //     println!("{:?}", users);
        // }

/*
        println!("running mio event loop in a new thread");
        let poll_clone = poll.clone();
        std::thread::spawn(move || {
            loop {
                let mut events = Events::with_capacity(10);
                match poll_clone.lock().unwrap().poll(&mut events, None) {
                    Err(err) => {
                        println!("error in event loop: {:?}", err);
                        break;
                    },
                    Ok(_) => {
                        for event in events.iter() {
                            if event.readiness().is_readable() {

                            } else if event.readiness().is_writable() {

                            }
                        }
                    }
                }
            }
        });

        println!("running tokio core in the main thread");
        loop { core.turn(None) }
*/


/*
        let mut chan_map = HashMap::new();
        let mut user_map = HashMap::new();

        let resp = slack_api::rtm::connect(&http_client, &api_tok);

        let url: String = resp.unwrap().url.unwrap();

        {
            let resp = slack_api::channels::list(
                &http_client,
                &api_tok,
                &slack_api::channels::ListRequest {
                    exclude_archived: Some(true),
                    exclude_members: Some(false),
                });
            match resp {
                Ok(slack_api::channels::ListResponse { channels: Some(chans), .. }) => {
                    for chan in chans {
                        chan_map.insert(chan.id.unwrap(), chan.name.unwrap());
                    }
                },
                _ => {
                    println!("Can't get channels: {:?}", resp);
                }
            }
        }

        {
            let resp = slack_api::users::list(
                &http_client,
                &api_tok,
                &slack_api::users::ListRequest {
                    presence: Some(true),
                });

            match resp {
                Ok(slack_api::users::ListResponse { members: Some(users), .. }) => {
                    for user in users {
                        user_map.insert(user.id.unwrap(), user.name.unwrap());
                    }
                }
                _ => {
                    println!("Can't get users: {:?}", resp);
                }
            }
        }

        let url = Url::parse(&url).unwrap();
        let domain = url.host_str().unwrap();

        println!("url: {:?}, domain: {:?}", url, domain);

        let addrs = url.to_socket_addrs().unwrap();
        let stream = connect_to_some(addrs, &url);

        let ws: WebSocket<TlsStream<TcpStream>> =
            tungstenite::client(tungstenite::handshake::client::Request::from(url.clone()), stream).unwrap().0;

        poll.register(
            &EventedFd(&ws.get_ref().get_ref().as_raw_fd()),
            Token(ws.get_ref().get_ref().as_raw_fd() as usize),
            Ready::readable() | Ready::writable(),
            PollOpt::edge()).unwrap();

        SlackConn {
            api_tok: api_tok,
            http_client: http_client,
            ws: ws,
            chans: chan_map,
            users: user_map,
            poll: poll,
        }
*/
    }
}

fn connect_to_some<A>(addrs: A, url: &Url) -> TlsStream<TcpStream>
    where A: Iterator<Item=SocketAddr>
{
    let domain = url.host_str().unwrap();
    for addr in addrs {
        println!("Trying to contact {} at {}...", url, addr);
        let raw_stream = TcpStream::connect(addr).unwrap();
        return wrap_stream(raw_stream, domain);
    }
    panic!("Unable to connect to {}", url)
}

fn wrap_stream(stream: TcpStream, domain: &str) -> TlsStream<TcpStream> {
    let connector = TlsConnector::builder().unwrap().build().unwrap();
    connector.connect(domain, stream).unwrap()
}
