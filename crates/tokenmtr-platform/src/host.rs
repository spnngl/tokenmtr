//! What this machine is: host name, OS and kernel, read once for the `tokenmtr_machine_info`
//! metric and the `machine` label.
//!
//! Linux reads `os-release` and `/proc`; macOS reads `SystemVersion.plist` and runs `uname -r`
//! and `hostname`; other systems only run `hostname`. The file parsers are pure and compiled on
//! every OS so tests and the `host_files` fuzz target reach them. Every value goes through
//! [`label`]: a metric label stays short and printable whatever the files hold.

use std::sync::OnceLock;

use quick_xml::Reader;
use quick_xml::escape::resolve_predefined_entity;
use quick_xml::events::Event;

/// Longest label value kept, in bytes; longer values are cut on a character boundary.
pub const MAX_LABEL_BYTES: usize = 128;

/// What the metrics say about this machine. Every field is a label value: trimmed, free of
/// control characters, at most [`MAX_LABEL_BYTES`] long, and empty when unknown.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostInfo {
    /// Host name.
    pub host_name: String,
    /// Linux: `os-release` `ID` (e.g. `ubuntu`, default `linux`); macOS: `macos`.
    pub os_id: String,
    /// Linux: `VERSION_ID`; macOS: `ProductVersion` (e.g. `15.5`).
    pub os_version: String,
    /// Linux: `PRETTY_NAME`, else `NAME`, else `Linux`; macOS: `ProductName ProductVersion`
    /// (e.g. `macOS 15.5`).
    pub os_name: String,
    /// Linux: `BUILD_ID`; macOS: `ProductBuildVersion` (e.g. `24F74`).
    pub os_build: String,
    /// Kernel release: Linux `/proc/sys/kernel/osrelease`, macOS `uname -r` (e.g. `24.5.0`).
    pub kernel_version: String,
}

impl HostInfo {
    /// Reads this machine's description. Blocking (files, and on macOS two short processes);
    /// best effort: whatever cannot be read stays empty. Prefer [`host_info`], which reads once.
    #[must_use]
    pub fn detect() -> Self {
        detect()
    }

    /// A Linux description from a parsed `os-release`, the kernel release and the host name,
    /// with the `os-release` defaults for a missing `ID` and `NAME`.
    #[must_use]
    pub fn linux(release: &OsRelease, kernel: &str, host: &str) -> Self {
        Self {
            host_name: label(host),
            os_id: first_label(&[release.id.as_str(), "linux"]),
            os_version: label(&release.version_id),
            os_name: first_label(&[release.pretty_name.as_str(), release.name.as_str(), "Linux"]),
            os_build: label(&release.build_id),
            kernel_version: label(kernel),
        }
    }

    /// A macOS description from a parsed `SystemVersion.plist`, the kernel release and the host
    /// name.
    #[must_use]
    pub fn macos(version: &SystemVersion, kernel: &str, host: &str) -> Self {
        let product = first_label(&[version.product_name.as_str(), "macOS"]);
        Self {
            host_name: label(host),
            os_id: "macos".to_owned(),
            os_version: label(&version.product_version),
            os_name: label(&format!("{product} {}", version.product_version)),
            os_build: label(&version.product_build_version),
            kernel_version: label(kernel),
        }
    }
}

/// This machine's description, read by the first call and shared by every later one.
/// Blocking on the first call only.
pub fn host_info() -> &'static HostInfo {
    static HOST: OnceLock<HostInfo> = OnceLock::new();
    HOST.get_or_init(HostInfo::detect)
}

#[cfg(target_os = "linux")]
fn detect() -> HostInfo {
    let release = ["/etc/os-release", "/usr/lib/os-release"]
        .into_iter()
        .find_map(read_lossy)
        .unwrap_or_default();
    HostInfo::linux(
        &parse_os_release(&release),
        &read_lossy("/proc/sys/kernel/osrelease").unwrap_or_default(),
        &read_lossy("/proc/sys/kernel/hostname").unwrap_or_default(),
    )
}

#[cfg(target_os = "macos")]
fn detect() -> HostInfo {
    let plist = read_lossy("/System/Library/CoreServices/SystemVersion.plist").unwrap_or_default();
    HostInfo::macos(
        &parse_system_version_plist(&plist),
        &stdout_of("uname", &["-r"]),
        &stdout_of("hostname", &[]),
    )
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn detect() -> HostInfo {
    HostInfo {
        host_name: label(&stdout_of("hostname", &[])),
        ..HostInfo::default()
    }
}

/// A file's text, invalid UTF-8 replaced; `None` when it cannot be read.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn read_lossy(path: &str) -> Option<String> {
    std::fs::read(path)
        .ok()
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
}

/// A successful command's standard output; empty when it cannot run or fails.
#[cfg(not(target_os = "linux"))]
fn stdout_of(program: &str, args: &[&str]) -> String {
    use crate::service::{CommandOutput, CommandRunner, SystemRunner};

    SystemRunner
        .run(program, args)
        .ok()
        .filter(CommandOutput::success)
        .map(|out| out.stdout)
        .unwrap_or_default()
}

/// `value` as a label value: control characters removed, trimmed, and cut to at most
/// [`MAX_LABEL_BYTES`] on a character boundary.
#[must_use]
pub fn label(value: &str) -> String {
    let clean: String = value.chars().filter(|c| !c.is_control()).collect();
    let clean = clean.trim();
    clean[..clean.floor_char_boundary(MAX_LABEL_BYTES)]
        .trim_end()
        .to_owned()
}

/// The first candidate whose [`label`] is not empty, else empty.
fn first_label(candidates: &[&str]) -> String {
    candidates
        .iter()
        .map(|candidate| label(candidate))
        .find(|value| !value.is_empty())
        .unwrap_or_default()
}

/// The `os-release` fields a [`HostInfo`] uses, unquoted and unescaped; empty when the file does
/// not set them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OsRelease {
    /// `ID`, e.g. `ubuntu`.
    pub id: String,
    /// `NAME`, e.g. `Ubuntu`.
    pub name: String,
    /// `PRETTY_NAME`, e.g. `Ubuntu 24.04.2 LTS`.
    pub pretty_name: String,
    /// `VERSION_ID`, e.g. `24.04`.
    pub version_id: String,
    /// `BUILD_ID`, e.g. `rolling`.
    pub build_id: String,
}

/// Parses an `os-release` file (freedesktop spec): `KEY=value` lines, values optionally in
/// single quotes (literal) or double quotes, backslash escaping the next character outside
/// single quotes; `#` comment lines, blank lines and lines without `=` are skipped, and a later
/// assignment wins. Never fails: unknown keys are ignored and missing ones stay empty.
#[must_use]
pub fn parse_os_release(text: &str) -> OsRelease {
    let mut release = OsRelease::default();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        let Some((key, raw)) = line.split_once('=') else {
            continue;
        };
        let field = match key.trim() {
            "ID" => &mut release.id,
            "NAME" => &mut release.name,
            "PRETTY_NAME" => &mut release.pretty_name,
            "VERSION_ID" => &mut release.version_id,
            "BUILD_ID" => &mut release.build_id,
            _ => continue,
        };
        *field = unquote(raw.trim());
    }
    release
}

/// A shell-style value: the text inside its quotes (the rest of the line when the closing quote
/// is missing), with backslash escapes resolved except inside single quotes.
fn unquote(raw: &str) -> String {
    let mut chars = raw.chars().peekable();
    let quote = chars.next_if(|c| matches!(c, '"' | '\''));
    let mut value = String::with_capacity(raw.len());
    while let Some(c) = chars.next() {
        match c {
            c if Some(c) == quote => break,
            '\\' if quote != Some('\'') => value.extend(chars.next()),
            c => value.push(c),
        }
    }
    value
}

/// The `SystemVersion.plist` fields a [`HostInfo`] uses; empty when the file does not set them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SystemVersion {
    /// `ProductName`, e.g. `macOS`.
    pub product_name: String,
    /// `ProductVersion`, e.g. `15.5`.
    pub product_version: String,
    /// `ProductBuildVersion`, e.g. `24F74`.
    pub product_build_version: String,
}

/// Parses macOS's `SystemVersion.plist`: the `<key>`/`<string>` pairs directly in the top-level
/// `<plist><dict>`, entities and CDATA resolved. Nested values and keys paired with another value
/// type are ignored. Never fails: ill-formed XML ends the parse with the pairs read so far.
#[must_use]
pub fn parse_system_version_plist(text: &str) -> SystemVersion {
    let mut reader = Reader::from_str(text);
    let mut dict = TopDict::default();
    // Depth of the innermost open element: 1 is the root, 3 a pair element in the top `<dict>`.
    let mut depth = 0_usize;
    while let Ok(event) = reader.read_event() {
        match event {
            Event::Start(e) => {
                depth += 1;
                dict.open(depth, e.local_name().as_ref());
            }
            Event::Empty(e) => {
                dict.open(depth + 1, e.local_name().as_ref());
                dict.close(depth + 1);
            }
            Event::End(_) => {
                dict.close(depth);
                depth = depth.saturating_sub(1);
            }
            Event::Text(e) => dict.push(&e.xml10_content()),
            Event::CData(e) => dict.push(&e.xml10_content()),
            Event::GeneralRef(e) => match e.resolve_char_ref() {
                Ok(Some(c)) => dict.push(c.encode_utf8(&mut [0; 4])),
                Ok(None) => dict.push(resolve_predefined_entity(&e).unwrap_or_default()),
                Err(_) => {}
            },
            Event::Eof => break,
            _ => {}
        }
    }
    dict.version
}

/// The kind of an element directly in the top-level `<dict>`.
#[derive(Debug, Clone, Copy)]
enum Slot {
    Key,
    String,
    /// Any other value type (`<integer>`, `<dict>`, `<true/>`…): its key gets no string.
    Other,
}

/// Parse state of [`parse_system_version_plist`].
#[derive(Debug, Default)]
struct TopDict {
    /// The open root element is `<plist>`.
    in_plist: bool,
    /// A `<dict>` is open directly in the root `<plist>`.
    in_dict: bool,
    /// The open pair element, if any, and its text so far.
    slot: Option<Slot>,
    text: String,
    /// The last key read, waiting for its value.
    key: Option<String>,
    version: SystemVersion,
}

impl TopDict {
    fn open(&mut self, depth: usize, name: &str) {
        match depth {
            1 => self.in_plist = name == "plist",
            2 => self.in_dict = self.in_plist && name == "dict",
            3 if self.in_dict => {
                self.slot = Some(match name {
                    "key" => Slot::Key,
                    "string" => Slot::String,
                    _ => Slot::Other,
                });
                self.text.clear();
            }
            _ => {}
        }
    }

    fn close(&mut self, depth: usize) {
        match depth {
            1 => self.in_plist = false,
            2 => self.in_dict = false,
            3 => match self.slot.take() {
                Some(Slot::Key) => self.key = Some(std::mem::take(&mut self.text)),
                Some(Slot::String) => {
                    if let Some(key) = self.key.take() {
                        self.set(&key);
                    }
                }
                Some(Slot::Other) => self.key = None,
                None => {}
            },
            _ => {}
        }
    }

    fn push(&mut self, text: &str) {
        if self.slot.is_some() {
            self.text.push_str(text);
        }
    }

    /// Stores the text of the closed `<string>` under `key` when it is a field we keep.
    fn set(&mut self, key: &str) {
        let field = match key {
            "ProductName" => &mut self.version.product_name,
            "ProductVersion" => &mut self.version.product_version,
            "ProductBuildVersion" => &mut self.version.product_build_version,
            _ => return,
        };
        *field = std::mem::take(&mut self.text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const UBUNTU: &str = include_str!("../tests/fixtures/os-release-ubuntu");
    const ARCH: &str = include_str!("../tests/fixtures/os-release-arch");
    const PLIST: &str = include_str!("../tests/fixtures/SystemVersion.plist");

    #[test]
    fn os_release_fixtures_describe_the_distribution() {
        let ubuntu = HostInfo::linux(&parse_os_release(UBUNTU), "6.8.0-60-generic\n", "box\n");
        assert_eq!(
            ubuntu,
            HostInfo {
                host_name: "box".into(),
                os_id: "ubuntu".into(),
                os_version: "24.04".into(),
                os_name: "Ubuntu 24.04.2 LTS".into(),
                os_build: String::new(),
                kernel_version: "6.8.0-60-generic".into(),
            }
        );
        let arch = parse_os_release(ARCH);
        assert_eq!(
            (arch.id.as_str(), arch.build_id.as_str()),
            ("arch", "rolling")
        );
        assert_eq!(arch.version_id, "");
    }

    #[test]
    fn os_release_quoting_escapes_and_comments() {
        let text = "\
# ID=commented
   # indented comment

ID=  'deb\\ian'  
NAME=\"Say \\\"hi\\\" \\\\ \\$HOME\"
PRETTY_NAME=plain\\ value
VERSION_ID=\"unterminated
no equals sign here
BUILD_ID=first
BUILD_ID=second
UNKNOWN=x
";
        assert_eq!(
            parse_os_release(text),
            OsRelease {
                id: "deb\\ian".into(),
                name: r#"Say "hi" \ $HOME"#.into(),
                pretty_name: "plain value".into(),
                version_id: "unterminated".into(),
                build_id: "second".into(),
            }
        );
    }

    #[test]
    fn os_release_defaults_apply_when_keys_are_missing() {
        let info = HostInfo::linux(&parse_os_release("NAME=Distro\n"), "", "");
        assert_eq!(info.os_id, "linux");
        assert_eq!(info.os_name, "Distro");
        let info = HostInfo::linux(&OsRelease::default(), "", "");
        assert_eq!(
            (info.os_id.as_str(), info.os_name.as_str()),
            ("linux", "Linux")
        );
        assert_eq!(info.os_version, "");
    }

    #[test]
    fn os_release_garbage_yields_nothing() {
        for junk in [
            "",
            "\0\u{1}\u{fffd}",
            "=",
            "==\n=\"",
            "\"",
            "\\",
            "ID",
            "ID=\\",
        ] {
            let release = parse_os_release(junk);
            assert!(
                release.name.is_empty() && release.version_id.is_empty(),
                "{junk:?}"
            );
        }
    }

    #[test]
    fn plist_fixture_describes_macos() {
        let info = HostInfo::macos(&parse_system_version_plist(PLIST), "24.5.0\n", "mac\n");
        assert_eq!(
            info,
            HostInfo {
                host_name: "mac".into(),
                os_id: "macos".into(),
                os_version: "15.5".into(),
                os_name: "macOS 15.5".into(),
                os_build: "24F74".into(),
                kernel_version: "24.5.0".into(),
            }
        );
    }

    #[test]
    fn plist_resolves_entities_and_cdata_and_skips_other_values() {
        let text = r"<plist><dict>
            <key>Nested</key><dict><key>ProductName</key><string>inner</string></dict>
            <key>ProductName</key><string>Mac &amp; &#x4F;S</string>
            <key>ProductVersion</key><integer>15</integer><string>orphan</string>
            <key>ProductBuildVersion</key><string><![CDATA[2<4]]></string>
        </dict></plist>";
        assert_eq!(
            parse_system_version_plist(text),
            SystemVersion {
                product_name: "Mac & OS".into(),
                product_version: String::new(),
                product_build_version: "2<4".into(),
            }
        );
    }

    #[test]
    fn plist_ignores_pairs_outside_the_top_dict_and_keeps_pairs_before_an_error() {
        let outside = "<dict><key>ProductName</key><string>x</string></dict>";
        assert_eq!(
            parse_system_version_plist(outside),
            SystemVersion::default()
        );
        let broken = "<plist><dict><key>ProductName</key><string>macOS</string><key>Pro</wrong>";
        assert_eq!(parse_system_version_plist(broken).product_name, "macOS");
        for junk in [
            "",
            "<",
            "<<>>",
            "&bogus;",
            "<plist><dict><key>",
            "\u{0}\u{ff}",
        ] {
            assert_eq!(
                parse_system_version_plist(junk),
                SystemVersion::default(),
                "{junk:?}"
            );
        }
    }

    #[test]
    fn labels_drop_control_characters_and_cap_on_a_char_boundary() {
        assert_eq!(label("  a\tb\u{7f}c\n"), "abc");
        let long = "é".repeat(100);
        let capped = label(&long);
        assert_eq!(capped.len(), MAX_LABEL_BYTES);
        assert!(capped.chars().all(|c| c == 'é'));
        assert_eq!(label(&("a".repeat(127) + "é")).len(), 127);
    }

    #[test]
    fn host_info_is_read_once() {
        assert!(std::ptr::eq(host_info(), host_info()));
        if cfg!(any(target_os = "linux", target_os = "macos")) {
            assert_ne!(host_info().os_id, "");
        }
    }
}
