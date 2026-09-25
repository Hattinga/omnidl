//! Gives the Windows programs their icon and version information (Explorer,
//! Start menu, installer). The resource script is generated so the version always
//! matches Cargo.toml.

fn main() {
    println!("cargo:rerun-if-changed=assets/omnidl.ico");
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let version = env!("CARGO_PKG_VERSION");
    let mut parts = version.split(['.', '-']).map(|p| p.parse::<u16>().unwrap_or(0));
    let (major, minor, patch) = (parts.next().unwrap_or(0), parts.next().unwrap_or(0), parts.next().unwrap_or(0));
    let icon = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets").join("omnidl.ico");
    let icon = icon.display().to_string().replace('\\', "\\\\");

    // Each program gets its own name, so Task Manager and Explorer tell them apart.
    for (bin, description) in [("omnidl", "omnidl"), ("omnidl-cli", "omnidl-cli (Terminal und Web-Interface)")] {
        let rc = format!(
            r#"1 ICON "{icon}"
1 VERSIONINFO
FILEVERSION {major},{minor},{patch},0
PRODUCTVERSION {major},{minor},{patch},0
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040704B0"
    BEGIN
      VALUE "CompanyName", "omnidl"
      VALUE "FileDescription", "{description}"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "{bin}"
      VALUE "OriginalFilename", "{bin}.exe"
      VALUE "ProductName", "omnidl"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x0407, 1200
  END
END
"#
        );
        let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap()).join(format!("{bin}.rc"));
        std::fs::write(&out, rc).unwrap();
        embed_resource::compile_for(&out, [bin], embed_resource::NONE).manifest_optional().unwrap();
    }
}
