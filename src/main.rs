// https://tools.ietf.org/rfc/rfc5128.txt
// https://blog.csdn.net/bytxl/article/details/44344855

use flexi_logger::*;
use hbb_common::{bail, config::RENDEZVOUS_PORT, ResultType};
use hbbs::{common::*, *};

const RMEM: usize = 0;

fn main() -> ResultType<()> {
    crate::common::anchor_to_exe_dir();
    let _logger = crate::common::init_file_logger("hbbs")?;
    let args = format!(
        "-c --config=[FILE] +takes_value 'Sets a custom config file'
        -b, --bind=[IP] 'Sets the IP address to bind to (default: all interfaces)'
        -p, --port=[NUMBER(default={RENDEZVOUS_PORT})] 'Sets the listening port'
        -s, --serial=[NUMBER(default=0)] '[DEPRECATED] Sets configure update serial number'
        -R, --rendezvous-servers=[HOSTS] '[DEPRECATED] Sets rendezvous servers, separated by comma'
        -u, --software-url=[URL] '[DEPRECATED] Sets download url of RustDesk software of newest version'
        -r, --relay-servers=[HOST] 'Sets the default relay servers, separated by comma'
        -M, --rmem=[NUMBER(default={RMEM})] 'Sets UDP recv buffer size, set system rmem_max first, e.g., sudo sysctl -w net.core.rmem_max=52428800. vi /etc/sysctl.conf, net.core.rmem_max=52428800, sudo sysctl –p'
        , --mask=[MASK] '[DEPRECATED] Determine if the connection comes from LAN, e.g. 192.168.0.0/16'
        -k, --key=[KEY] 'Only allow the client with the same key'
        , --api-listen=[ADDR(default=0.0.0.0:21114)] 'Sets the account API and web console listen address, \"off\" to disable'
        , --api-db=[FILE(default=api_v1.sqlite3)] 'Sets the API account database file'
        , --console-listen=[ADDR] 'Sets a separate listen address for the web console only; the API address then serves client endpoints only'
        , --tls-cert=[FILE] 'Enables HTTPS on the API/console listeners with this PEM certificate (requires --tls-key)'
        , --tls-key=[FILE] 'Sets the PEM private key matching --tls-cert'",
    );
    init_args(&args, "hbbs", "RustDesk ID/Rendezvous Server");
    let api_listen = get_arg_or("api-listen", "0.0.0.0:21114".to_owned());
    if api_listen != "off" {
        let api_db = get_arg_or("api-db", "api_v1.sqlite3".to_owned());
        // The rendezvous needs the account database path for the
        // self-connection check (login token -> device id); the API
        // thread owns the pool, so the path is shared via the env.
        std::env::set_var("API_DB_PATH", api_db.clone());
        let key_override = get_arg("key");
        let console_listen = get_arg("console-listen");
        let tls_cert = get_arg("tls-cert");
        let tls_key = get_arg("tls-key");
        std::thread::spawn(move || {
            if let Err(e) = hbbs::api::run(
                &api_listen,
                &api_db,
                &key_override,
                &console_listen,
                &tls_cert,
                &tls_key,
            ) {
                eprintln!("api service failed to start: {e}");
            }
        });
    }
    let port = get_arg_or("port", RENDEZVOUS_PORT.to_string()).parse::<i32>()?;
    if port < 3 {
        bail!("Invalid port");
    }
    let bind_addr = parse_bind_address(&get_arg("bind"))?;
    let rmem = get_arg("rmem").parse::<usize>().unwrap_or(RMEM);
    let serial: i32 = get_arg("serial").parse().unwrap_or(0);
    crate::common::check_software_update();
    RendezvousServer::start_with_bind(
        bind_addr,
        port,
        serial,
        &get_arg_or("key", "-".to_owned()),
        rmem,
    )?;
    Ok(())
}
