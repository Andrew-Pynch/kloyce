pub fn install() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let home = std::env::var("HOME")?;

    // Create log directory
    let log_dir = format!("{home}/Library/Logs/kloyce");
    std::fs::create_dir_all(&log_dir)?;

    // Install LaunchAgent plist
    let agents_dir = format!("{home}/Library/LaunchAgents");
    std::fs::create_dir_all(&agents_dir)?;

    let plist_content = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.kloyce.daemon</string>
    <key>ProgramArguments</key>
    <array>
        <string>{home}/.cargo/bin/kloyce</string>
        <string>daemon</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>{home}/Library/Logs/kloyce/stdout.log</string>
    <key>StandardErrorPath</key>
    <string>{home}/Library/Logs/kloyce/stderr.log</string>
    <key>EnvironmentVariables</key>
    <dict>
        <key>RUST_LOG</key>
        <string>kloyce=info</string>
        <key>PATH</key>
        <string>/opt/homebrew/bin:/usr/local/bin:{home}/.local/bin:/usr/bin:/bin:/usr/sbin:/sbin</string>
    </dict>
</dict>
</plist>
"#
    );

    let plist_path = format!("{agents_dir}/com.kloyce.daemon.plist");
    std::fs::write(&plist_path, plist_content)?;
    println!("Installed LaunchAgent to {plist_path}");

    println!("\nTo load the service:");
    println!("  launchctl load {plist_path}");
    println!("\nTo unload:");
    println!("  launchctl unload {plist_path}");
    println!("\nLogs:");
    println!("  tail -f {log_dir}/stdout.log");
    println!("  tail -f {log_dir}/stderr.log");

    Ok(())
}
