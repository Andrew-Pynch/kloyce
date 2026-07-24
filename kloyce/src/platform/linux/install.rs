pub fn install() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let home = std::env::var("HOME")?;

    // Install systemd service
    let service_dir = format!("{home}/.config/systemd/user");
    std::fs::create_dir_all(&service_dir)?;
    let service_content = format!(
        r#"[Unit]
Description=Kloyce Speech-to-Text Daemon
After=graphical-session.target pipewire.service

[Service]
Type=simple
ExecStart={home}/.cargo/bin/kloyce daemon
Restart=on-failure
RestartSec=2

[Install]
WantedBy=graphical-session.target
"#
    );
    std::fs::write(format!("{service_dir}/kloyce.service"), service_content)?;
    println!("Installed systemd service to {service_dir}/kloyce.service");

    // Append hyprland binding
    let bindings_path = format!("{home}/.config/hypr/bindings.conf");
    if std::path::Path::new(&bindings_path).exists() {
        let content = std::fs::read_to_string(&bindings_path)?;
        if !content.contains("kloyce") {
            let binding = "\n# Kloyce voice-to-text\nbindd = SUPER, R, Voice input (toggle), exec, kloyce-ctl toggle\nbindd = SUPER, E, Voice input (toggle + enter), exec, kloyce-ctl toggle-enter\nbindd = SUPER SHIFT, E, Copy latest voice transcript with audio metadata, exec, kloyce-ctl copy-plus\n";
            std::fs::write(&bindings_path, format!("{content}{binding}"))?;
            println!("Appended Kloyce Hyprland bindings to {bindings_path}");
        } else {
            println!("Hyprland binding already exists, skipping");
        }
    } else {
        println!("Warning: {bindings_path} not found, skipping hyprland binding");
    }

    println!("\nTo enable the service:");
    println!("  systemctl --user daemon-reload");
    println!("  systemctl --user enable --now kloyce");

    Ok(())
}
