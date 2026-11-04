
use irust_net::{ConnectOptions, Connection};

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

    conn.write_line("NICK irust-demo-abcX").await?;
    conn.write_line("USER irust 0 * :irust connect demo").await?;

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, conn.read_line()).await {
            Ok(Some(Ok(line))) => {
                println!("< {line}");
                if line.contains(" 001 ") {
                    println!("[demo] got RPL_WELCOME, registration succeeded");
                }
            }
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
        }
    }

    let _ = conn.write_line("QUIT :demo done").await;
    Ok(())
}
