//! A local stand-in for a ModMed practice: medical-common's mock SMART +
//! FHIR server, serving one synthetic patient, and ModMed's endpoint
//! directory serving synthetic practices, for dogfooding the installed
//! binary (`make mock`; CONTRIBUTING.md shows how to point modmed at them).
//! Every value they serve is synthetic.

#[path = "../tests/support/directory.rs"]
mod directory;

use pk_med_fhir::test_util::{MockServer, CLIENT_ID, CLIENT_SECRET};

fn main() {
    let mock = MockServer::start();
    let directory = directory::DirectoryServer::start();
    println!("FHIR base:     {}", mock.fhir_base());
    println!("directory:     {}", directory.url());
    println!("client pair:   {CLIENT_ID}:{CLIENT_SECRET}");
    println!("Ctrl-C stops them.");
    loop {
        std::thread::park();
    }
}
