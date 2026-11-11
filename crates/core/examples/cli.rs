//! Tries the backend from a computer: `cli code` asks for a sign-in code (and signs nothing in).

use crunchy_core::auth;
use crunchy_core::platform::Platform;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let dir = std::env::temp_dir().join("crunchy-cli");
    match std::env::args().nth(1).as_deref() {
        Some("code") => {
            let platform = Platform::load(&dir, false).await?;
            let client = platform.http_client()?;
            let code = auth::request_code(&client, &platform).await?;
            println!("code {} for {}, every {:?}, expires in {:?}", code.user_code, auth::ACTIVATE_URL, code.interval, code.expires_in);
        }
        _ => eprintln!("usage: cli code"),
    }
    Ok(())
}
