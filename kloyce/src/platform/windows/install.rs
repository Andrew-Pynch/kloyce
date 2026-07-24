use std::process::Command;

pub fn install() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Get the path to the kloyce binary
    let exe_path =
        std::env::current_exe().map_err(|e| format!("Failed to get current exe path: {e}"))?;
    let exe_dir = exe_path
        .parent()
        .ok_or("Failed to get exe parent directory")?;
    let kloyce_bin = exe_dir.join("kloyce.exe");

    if !kloyce_bin.exists() {
        return Err(format!(
            "kloyce.exe not found at {}. Build it first with: cargo build --release",
            kloyce_bin.display()
        )
        .into());
    }

    // Add auto-start registry entry
    let daemon_cmd = format!("\"{}\" daemon", kloyce_bin.display());
    let status = Command::new("reg")
        .args([
            "add",
            r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
            "/v",
            "Kloyce",
            "/t",
            "REG_SZ",
            "/d",
            &daemon_cmd,
            "/f",
        ])
        .status()
        .map_err(|e| format!("Failed to run reg command: {e}"))?;

    if !status.success() {
        return Err("Failed to add registry auto-start entry".into());
    }
    println!("Added auto-start registry entry for Kloyce daemon");

    // Create config directory
    let appdata = std::env::var("APPDATA").unwrap_or_else(|_| {
        let profile = std::env::var("USERPROFILE").unwrap_or_else(|_| "C:\\Users\\Default".into());
        format!("{profile}\\AppData\\Roaming")
    });
    let config_dir = std::path::PathBuf::from(&appdata).join("kloyce");
    std::fs::create_dir_all(&config_dir)?;
    println!("Config directory: {}", config_dir.display());

    // Print hotkey setup instructions
    println!();
    println!("To set up a global hotkey (e.g., Win+R):");
    println!(
        "  Option 1: Create a shortcut to 'kloyce-ctl.exe toggle' and assign a keyboard shortcut"
    );
    println!("  Option 2: Use AutoHotkey with:");
    println!("    #r::Run, kloyce-ctl.exe toggle");
    println!();
    println!("The daemon will start automatically on login.");
    println!("To start it now, run: kloyce.exe daemon");

    Ok(())
}
