//! Print every report chronyd gives, decoded: a check of the client
//! against a live chronyd.  Its socket needs chronyd's user or root;
//! its UDP port, the reports opened to it (`opencommands`).
//!
//!     cargo run --example reports [-- /run/chrony/chronyd.sock | 127.0.0.1]

use std::env;
use std::error::Error;

use chronymon_control::client::Client;
use chronymon_control::client::DEFAULT_SOCKET;
use chronymon_control::client::Server;
use chronymon_control::report::Address;

fn main() -> Result<(), Box<dyn Error>> {
    let server: Server = env::args()
        .nth(1)
        .unwrap_or_else(|| DEFAULT_SOCKET.to_owned())
        .parse()?;
    let mut client = Client::connect(&server)?;
    println!("{:#?}", client.tracking()?);
    // chronymond does not read it, so its `opencommands` leaves it out.
    println!("{:#?}", client.activity());
    for index in 0..client.source_count()? {
        let source = client.source_data(index)?;
        println!("{source:#?}");
        println!("{:#?}", client.source_stats(index)?);
        println!("{:#?}", client.select_data(index)?);
        if let Address::Ip(address) = source.address {
            println!("name: {}", client.source_name(address)?);
            println!("{:#?}", client.ntp_data(address)?);
        }
    }
    Ok(())
}
