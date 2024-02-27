use jack::{Client, ClientOptions, PortFlags};

fn main() {
    let mut c = Client::new("foo", ClientOptions::empty()).unwrap();

    for p in c.0.ports(None, None, PortFlags::empty()) {
        println!("port {}", p);
    }
    println!("Hello, world!");
}
