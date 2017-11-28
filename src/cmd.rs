use utils;

pub enum Cmd<'input> {
    Connect(Option<(&'input str, u16)>),
    Join {
        chans: Vec<&'input str>,
    },
    Msg {
        target: &'input str,
        msg: &'input str,
    },
    Me {
        msg: &'input str,
    },
    Away {
        reason: Option<&'input str>,
    },
    Close,
    Nick {
        nick: &'input str,
    },
    Reload,
    Names,
    Clear,
    Switch {
        str: &'input str,
    },
    Ignore,
}

impl<'input> Cmd<'input> {
    pub fn parse(input: &'input str) -> Result<Cmd<'input>, String> {
        let words: Vec<&'input str> = input.split_whitespace().into_iter().collect();
        if words.is_empty() {
            return Err(format!("Can't parse command: {:?}", input));
        }
        let cmd = words[0];
        let args = &words[1..];
        match cmd {
            "connect" => {
                if args.is_empty() {
                    return Ok(Cmd::Connect(None));
                } else if args.len() != 1 {
                    return Err(format!(
                        "/connect: Need one <host>:<port> argument. Got: {:?}",
                        args
                    ));
                }
                let arg = args[0];
                match arg.find(':')
                    .map(|split| (&arg[0..split], &arg[split + 1..]))
                {
                    None =>
                        Err(format!("/connect: Can't parse <host>:<port>: {:?}", arg)),
                    Some((serv_name, serv_port)) =>
                        match serv_port.parse::<u16>() {
                            Err(err) =>
                                Err(format!("/connect: Can't parse port {}: {}", serv_port, err)),
                            Ok(serv_port) =>
                                Ok(Cmd::Connect(Some((serv_name, serv_port)))),
                        },
                }
            }

            "join" =>
                if !args.is_empty() {
                    Err("/join: Need at least one argument".to_owned())
                } else {
                    Ok(Cmd::Join {
                        chans: args.to_vec(),
                    })
                },

            "msg" => {
                if args.len() < 2 {
                    return Err("/msg: Need at least two arguments".to_owned());
                }
                let mut word_indices = utils::split_whitespace_indices(input);
                word_indices.next(); // "/msg"
                word_indices.next(); // target
                let msg_begins = word_indices.next().unwrap();
                Ok(Cmd::Msg {
                    target: args[0],
                    msg: &input[msg_begins..],
                })
            }

            "me" => {
                if args.is_empty() {
                    return Err("/me: Need at least one argument".to_owned());
                }
                let mut word_indices = utils::split_whitespace_indices(input);
                word_indices.next(); // "/me"
                let msg_begins = word_indices.next().unwrap();
                Ok(Cmd::Me {
                    msg: &input[msg_begins..],
                })
            }

            "away" =>
                if args.is_empty() {
                    Ok(Cmd::Away { reason: None })
                } else {
                    let mut word_indices = utils::split_whitespace_indices(input);
                    word_indices.next(); // "/away"
                    let msg_begins = word_indices.next().unwrap();
                    Ok(Cmd::Away {
                        reason: Some(&input[msg_begins..]),
                    })
                },

            "close" =>
                Ok(Cmd::Close),

            "nick" =>
                if args.len() != 1 {
                    Err(format!("/nick: Need single argument. Got: {:?}", args))
                } else {
                    Ok(Cmd::Nick { nick: args[0] })
                },

            "reload" =>
                Ok(Cmd::Reload),

            "names" =>
                Ok(Cmd::Names),

            "clear" =>
                Ok(Cmd::Clear),

            "switch" =>
                if args.len() != 1 {
                    Err(format!("/switch: Need single argument. Got: {:?}", args))
                } else {
                    Ok(Cmd::Switch { str: args[0] })
                },

            "ignore" =>
                Ok(Cmd::Ignore),

            _ =>
                Err(format!("Unknown command: {}", cmd)),
        }
    }
}
