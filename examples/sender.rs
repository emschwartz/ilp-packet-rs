extern crate bytes;
extern crate chrono;
extern crate env_logger;
extern crate futures;
extern crate interledger;
extern crate ring;
extern crate tokio;

use interledger::plugin::btp::connect_to_moneyd;
use tokio::prelude::*;
// use ilp::spsp::pay;
use interledger::spsp::connect_async;

fn main() {
    env_logger::init();

    let future = connect_to_moneyd()
        .map_err(|err| {
            println!("{}", err);
        })
        .and_then(move |plugin| {
            println!("Conected sender");

            // pay(plugin, "http://localhost:3000", 100)
            //   .and_then(|amount_sent| {
            //     println!("Sent {}", amount_sent);
            //     Ok(())
            //   })

            connect_async(plugin, "http://localhost:3000/spsp/bob")
                .map_err(|err| {
                    println!("Error connecting to SPSP server {:?}", err);
                })
                .and_then(|connection| {
                    println!("Creating new stream and sending money");
                    let mut stream = connection.create_stream();
                    stream
                        .money
                        .clone()
                        .send(1000)
                        .and_then(move |_| {
                            println!("Sent money");
                            let bytes = b"hey there";
                            stream
                                .data
                                .write(&bytes[..])
                                .map_err(|err| {
                                    println!("Error writing {}", err);
                                })
                                .unwrap();
                            println!("Sent data");
                            println!("Closing connection");
                            connection.close()
                        })
                        .and_then(|_| {
                            println!("Closed connection");
                            Ok(())
                        })
                })
        })
        .then(|_| Ok(()));

    tokio::runtime::run(future);
}
