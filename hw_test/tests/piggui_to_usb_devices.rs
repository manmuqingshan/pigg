/// These tests test connecting to USB connected porky devices by USB and TCP, from the piggui
/// binary using CLIP options
///
#[path = "../../piggui/tests/support.rs"]
mod support;

use crate::support::{kill, run, wait_for_stdout};
use pignet::usb_host;
use serial_test::serial;

#[tokio::test]
#[serial]
async fn usb_discover_and_connect_usb() {
    let serials = usb_host::get_serials()
        .await
        .expect("No usb porky attached");

    let number = serials.len();
    assert!(number > 0, "Could not find by USB to connect to by USB");

    for serial in serials {
        let mut piggui = run("piggui", vec!["--usb".to_string(), serial], None);

        wait_for_stdout(&mut piggui, "Connected to hardware")
            .expect("Did not get connected message");

        kill(&mut piggui);
    }

    println!(
        "Tested piggui USB connection to {} USB discovered devices",
        number
    );
}

//reconnect usb (kill and restart)
