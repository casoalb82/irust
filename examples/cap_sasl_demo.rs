
use irust_caps::builtin::{MessageTags, Sasl};
use irust_caps::CapabilityRegistry;
use irust_net::{ConnectOptions, Connection};
use irust_proto::parse;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut conn = Connection::connect(ConnectOptions {
        host: "irc.libera.chat",
        port: 6697,
        tls: true,
        verify_tls: true,
        ip_version: None,
        client_auth: None,
        tag: "libera-demo",
        tls_fingerprint: None,
    })
    .await?;
    println!("[demo] TLS connected");

    let mut registry = CapabilityRegistry::new(vec![
        Box::new(Sasl::new("irust-demo", "not-a-real-password")),
        Box::new(MessageTags),
    ]);

    for line in registry.start() {
        println!("> {line}");
        conn.write_line(&line).await?;
    }

    conn.write_line("NICK irust-demo-abcX").await?;
    conn.write_line("USER irust 0 * :irust CAP/SASL demo").await?;

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            println!("[demo] deadline hit");
            break;
        }

        let line = match tokio::time::timeout(remaining, conn.read_line()).await {
            Ok(Some(Ok(line))) => line,
            Ok(Some(Err(err))) => {
                eprintln!("[demo] read error: {err}");
                break;
            }
            Ok(None) => {
                println!("[demo] server closed the connection");
                break;
            }
            Err(_) => {
                println!("[demo] timed out waiting for more lines");
                break;
            }
        };
        println!("< {line}");

        let Some(msg) = parse(&line) else { continue };

        if msg.command == "001" {
            println!(
                "[demo] RPL_WELCOME received - active caps: {:?}",
                registry.active_caps()
            );
            break;
        }

        for reply in registry.handle_message(&msg) {
            println!("> {reply}");
            conn.write_line(&reply).await?;
        }
    }

    let _ = conn.write_line("QUIT :demo done").await;
    Ok(())
}
