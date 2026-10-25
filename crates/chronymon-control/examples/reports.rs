//! Print every report chronyd gives, decoded: a check of the client
//! against a live chronyd.  Needs access to its socket, so the
//! `_chrony` group or root.
//!
//!     cargo run --example reports [-- /run/chrony/chronyd.sock]

use std::env;
use std::error::Error;
use std::path::PathBuf;

use chronymon_control::client::Client;
use chronymon_control::client::DEFAULT_SOCKET;
use chronymon_control::report::Address;

fn main() -> Result<(), Box<dyn Error>> {
    let socket = env::args_os()
        .nth(1)
        .map_or_else(|| PathBuf::from(DEFAULT_SOCKET), PathBuf::from);
    let mut client = Client::connect(&socket)?;
    println!("{:#?}", client.tracking()?);
    println!("{:#?}", client.activity()?);
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
