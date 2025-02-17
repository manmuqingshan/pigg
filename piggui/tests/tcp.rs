use serial_test::serial;
use support::{ip_port, kill, run, wait_for_stdout};

mod support;

#[test]
#[serial]
fn connect_via_ip() {
    let mut piglet = run("piglet", vec![], None);
    let line = wait_for_stdout(&mut piglet, "ip:").expect("Could not get IP address");
    let (a, p) = ip_port(&line);

    let mut piggui = run(
        "piggui",
        vec!["--ip".to_string(), format!("{}:{}", a, p)],
        None,
    );

    wait_for_stdout(&mut piggui, "Connected to hardware").expect("Did not get connected message");

    kill(&mut piggui);
    kill(&mut piglet);
}
