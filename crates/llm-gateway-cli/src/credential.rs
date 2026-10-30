//! `b10x-llm-gateway credential set` (`spec/domains/deployment.yaml` `CredentialSetCommand`):
//! stores one upstream key in the file the deployment already names for it, so that the next
//! start reads it.
//!
//! The target comes from the document only ([`crate::load`]: the deployment document and the
//! routes document it names), and loading it reads no credential, owner secret or key. The key
//! is read from a hidden prompt on `/dev/tty`, or from standard input with `--stdin`, and held to
//! the reader's token rule. Unless `--no-probe`, one `GET <base path>/models` is sent with the
//! entry's own credential header and static headers over the client the gateway reaches hosted
//! endpoints with ([`HostedEndpoint::probe`]). The key is then written to a temporary file in
//! the target's directory, read back through the server's own trusted-file reader and token rule
//! ([`read_token`]), and only then renamed over the target. No line this writes carries the key.

use crate::{
    cli::{CredentialCli, CredentialCommand, SetArgs},
    config::{Deployment, load},
    hosted::{EndpointCredential, HostedEndpoint},
    keys::read_token,
    refusal::{CredentialRefusal, Refusal, StartupRefusal},
    relaying::PodConnector,
    trusted,
};
use llm_gateway::{AuthKind, CredentialHeader, StaticHeader, TargetBearer};
use rustix::{
    fs::{Mode, OFlags, open},
    process::geteuid,
    termios::{LocalModes, OptionalActions, SpecialCodeIndex, Termios, tcgetattr, tcsetattr},
};
use std::{
    ffi::OsString,
    fmt,
    fs::{self, File},
    io::{self, Read, Write},
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    process::ExitCode,
};

/// The longest key: the reader's token bound.
pub const MAX_KEY_BYTES: usize = 4096;

/// Why the command refused: one of its own [`CredentialRefusal`]s, or a [`StartupRefusal`] the
/// server's own loader or reader gave. The message never quotes the key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialFailure {
    code: &'static str,
    message: String,
}

impl CredentialFailure {
    fn new(kind: CredentialRefusal, message: impl Into<String>) -> Self {
        Self {
            code: kind.code(),
            message: message.into().replace(['\n', '\r'], " "),
        }
    }

    pub fn code(&self) -> &'static str {
        self.code
    }
}

impl From<Refusal> for CredentialFailure {
    fn from(refusal: Refusal) -> Self {
        Self {
            code: refusal.code(),
            message: refusal.message().to_owned(),
        }
    }
}

impl fmt::Display for CredentialFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

/// The key, overwritten when dropped; it has no `Display` and its `Debug` shows nothing.
struct Key(Vec<u8>);

impl fmt::Debug for Key {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Key([REDACTED])")
    }
}

impl Drop for Key {
    fn drop(&mut self) {
        self.0.fill(0);
        std::hint::black_box(&self.0);
    }
}

/// Where a key goes and how its endpoint is reached, read from the document only.
#[derive(Debug)]
pub struct CredentialTarget {
    /// What the messages name: `hosted.<alias>.credential_file` or
    /// `routes.<endpoint>.auth.secret`.
    pub key: String,
    /// The file the server reads the key from.
    pub path: PathBuf,
    base_url: String,
    auth_kind: AuthKind,
    credential_header: Option<CredentialHeader>,
    headers: Vec<StaticHeader>,
}

/// What `--model` or `--route` names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetKind {
    Model,
    Route,
}

/// How a refusal names the value of `flag`: never quoted, "the given name". A name the
/// document does not declare may be a key given where the alias belongs, and a generated key
/// (48 hex characters, say) has the shape of a plain name, so no shape rule tells them apart.
fn named(flag: &str) -> String {
    format!("{flag} (the given name)")
}

/// The target `name` of `kind` in `deployment`, read from the document only: no file is opened.
///
/// # Errors
/// `credential:unknown-target`, `credential:not-hosted`, `credential:anonymous`, or
/// `config:value` for a route's `~/` path without an absolute `home`.
pub fn target(
    deployment: &Deployment,
    kind: TargetKind,
    name: &str,
    home: Option<&Path>,
) -> Result<CredentialTarget, CredentialFailure> {
    let anonymous = |at: &str| {
        CredentialFailure::new(
            CredentialRefusal::Anonymous,
            format!("{at} is an anonymous endpoint: it takes no key"),
        )
    };
    match kind {
        TargetKind::Model => {
            if let Some(hosted) = deployment.hosted.get(name) {
                let at = format!("hosted.{name}");
                let path = hosted
                    .credential_file
                    .clone()
                    .filter(|_| hosted.auth_kind != AuthKind::Anonymous)
                    .ok_or_else(|| anonymous(&at))?;
                return Ok(CredentialTarget {
                    key: format!("{at}.credential_file"),
                    path,
                    base_url: hosted.base_url.clone(),
                    auth_kind: hosted.auth_kind,
                    credential_header: hosted.credential_header.clone(),
                    headers: hosted.headers.clone(),
                });
            }
            if deployment.bedrock.contains_key(name) {
                return Err(CredentialFailure::new(
                    CredentialRefusal::NotHosted,
                    format!(
                        "hosted.{name} is a Bedrock model: it is signed with container \
                         credentials, not a stored key"
                    ),
                ));
            }
            if deployment.models.contains_key(name) {
                return Err(CredentialFailure::new(
                    CredentialRefusal::NotHosted,
                    format!(
                        "models.{name} is a Runpod model: its vllm_api_key_file is not an \
                         upstream key"
                    ),
                ));
            }
            let hint = if deployment.routes.routes().any(|route| route.name() == name) {
                "; it is a route of the routes document, stored with --route"
            } else {
                ""
            };
            Err(CredentialFailure::new(
                CredentialRefusal::UnknownTarget,
                format!(
                    "{} is no [hosted.*] or [models.*] alias{hint}",
                    named("--model")
                ),
            ))
        }
        TargetKind::Route => {
            let unknown = || {
                CredentialFailure::new(
                    CredentialRefusal::UnknownTarget,
                    format!("{} is no route of the routes document", named("--route")),
                )
            };
            let route = deployment
                .routes
                .routes()
                .find(|route| route.name() == name)
                .ok_or_else(unknown)?;
            let endpoint = deployment.routes.endpoint_of(route)?;
            let at = format!("routes.{}", endpoint.name());
            let secret = endpoint.secret().ok_or_else(|| anonymous(&at))?;
            let key = format!("{at}.auth.secret");
            let path = secret.resolve(home).ok_or_else(|| {
                Refusal::new(
                    StartupRefusal::ConfigValue,
                    format!(
                        "{key} names a file:~/ path, and HOME is not set to an absolute directory"
                    ),
                )
            })?;
            Ok(CredentialTarget {
                key,
                path,
                base_url: endpoint.base_url().to_owned(),
                auth_kind: endpoint.auth_kind(),
                credential_header: endpoint.credential_header().cloned(),
                headers: endpoint.static_headers()?,
            })
        }
    }
}

/// The key of `bytes`, read up to and without its terminator: one printable ASCII token of
/// 1..=[`MAX_KEY_BYTES`] bytes, the reader's token rule. One final `\n` is the terminator; any
/// other CR or LF is refused.
///
/// # Errors
/// `credential:input-empty`, `credential:input-too-large` or `credential:input-not-a-token`,
/// never quoting a byte.
pub fn check_key(bytes: &[u8]) -> Result<Vec<u8>, CredentialFailure> {
    let key = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    if key.is_empty() {
        return Err(CredentialFailure::new(
            CredentialRefusal::InputEmpty,
            "the key is empty",
        ));
    }
    if key.len() > MAX_KEY_BYTES {
        return Err(CredentialFailure::new(
            CredentialRefusal::InputTooLarge,
            format!("the key exceeds {MAX_KEY_BYTES} bytes"),
        ));
    }
    if !key.iter().all(u8::is_ascii_graphic) {
        return Err(CredentialFailure::new(
            CredentialRefusal::InputNotAToken,
            "the key must be one line holding one printable ASCII token: no space, tab, CR, \
             second LF, control or non-ASCII byte, leading or trailing whitespace included",
        ));
    }
    Ok(key.to_vec())
}

/// Reads the key from standard input: at most one terminator past the bound is read.
fn read_stdin() -> Result<Key, CredentialFailure> {
    // Read in place into one allocation taken before the first byte, so it never reallocates
    // and leaves no copy of the key in freed memory. Its length stays its capacity, so the
    // `Key` overwrites every byte of it when dropped, whatever a read wrote past what it read.
    let mut bytes = Key(vec![0; MAX_KEY_BYTES + 2]);
    let sized = bytes.0.capacity();
    let mut input = io::stdin().lock();
    let mut filled = 0;
    while filled < bytes.0.len() {
        match input.read(&mut bytes.0[filled..]) {
            Ok(0) => break,
            Ok(read) => filled += read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => {
                return Err(CredentialFailure::new(
                    CredentialRefusal::InputUnreadable,
                    format!("standard input: {error}"),
                ));
            }
        }
    }
    debug_assert_eq!(
        (bytes.0.len(), bytes.0.capacity()),
        (sized, sized),
        "a key read from standard input reallocated, leaving a copy of it in freed memory"
    );
    Ok(Key(check_key(&bytes.0[..filled])?))
}

/// Puts the terminal's saved modes back when dropped, on every path out of the prompt.
struct Restore<'a> {
    tty: &'a File,
    saved: Termios,
}

impl Drop for Restore<'_> {
    fn drop(&mut self) {
        let _ = tcsetattr(self.tty, OptionalActions::Flush, &self.saved);
    }
}

/// Reads the prompt's line from `reader` a byte at a time: Enter ends it; Ctrl-C, Ctrl-D or the
/// end of input before Enter (a hang-up) cancels, and a partial line is never taken; Backspace
/// deletes and Ctrl-U clears.
fn read_hidden_line(reader: &mut impl Read) -> Result<Key, CredentialFailure> {
    let cancelled = || {
        CredentialFailure::new(
            CredentialRefusal::InputCancelled,
            "the prompt was cancelled",
        )
    };
    // One byte past the bound is all a line ever holds, so it never reallocates and leaves no
    // copy of the key in freed memory; the `Key` overwrites it when dropped.
    let mut line = Key(Vec::with_capacity(MAX_KEY_BYTES + 1));
    let sized = line.0.capacity();
    let mut byte = [0_u8; 1];
    loop {
        match reader.read(&mut byte) {
            Ok(0) => return Err(cancelled()),
            Ok(_) => match byte[0] {
                b'\r' | b'\n' => return Ok(Key(check_key(&line.0)?)),
                0x03 | 0x04 => return Err(cancelled()),
                0x7f | 0x08 => {
                    if let Some(last) = line.0.last_mut() {
                        *last = 0;
                        line.0.pop();
                    }
                }
                0x15 => {
                    line.0.fill(0);
                    line.0.clear();
                }
                other => {
                    // One byte past the bound is enough to refuse it as too large.
                    if line.0.len() <= MAX_KEY_BYTES {
                        line.0.push(other);
                        debug_assert_eq!(
                            line.0.capacity(),
                            sized,
                            "a key reallocated as it was typed, leaving a copy of it in freed memory"
                        );
                    }
                }
            },
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => {
                return Err(CredentialFailure::new(
                    CredentialRefusal::InputUnreadable,
                    format!("/dev/tty: {error}"),
                ));
            }
        }
    }
}

/// Reads the key from a hidden prompt on `/dev/tty`: echo, canonical input and the signal keys
/// are off while it is read and restored on every path. Enter ends the key; Ctrl-C, Ctrl-D or a
/// hang-up cancels; Backspace deletes and Ctrl-U clears.
fn read_terminal(target: &CredentialTarget) -> Result<Key, CredentialFailure> {
    let no_terminal = |detail: &dyn fmt::Display| {
        CredentialFailure::new(
            CredentialRefusal::NoTerminal,
            format!("/dev/tty: {detail}; pass the key on standard input with --stdin"),
        )
    };
    let tty = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .map_err(|error| no_terminal(&error))?;
    let saved = tcgetattr(&tty).map_err(|errno| no_terminal(&errno))?;
    let mut hidden = saved.clone();
    hidden.local_modes.remove(
        LocalModes::ECHO
            | LocalModes::ECHONL
            | LocalModes::ICANON
            | LocalModes::ISIG
            | LocalModes::IEXTEN,
    );
    hidden.special_codes[SpecialCodeIndex::VMIN] = 1;
    hidden.special_codes[SpecialCodeIndex::VTIME] = 0;
    let mut writer = &tty;
    write!(
        writer,
        "b10x-llm-gateway: key for {} (input hidden): ",
        target.key
    )
    .and_then(|()| writer.flush())
    .map_err(|error| no_terminal(&error))?;
    tcsetattr(&tty, OptionalActions::Flush, &hidden).map_err(|errno| no_terminal(&errno))?;
    let restore = Restore { tty: &tty, saved };
    let outcome = read_hidden_line(&mut &tty);
    drop(restore);
    drop(writeln!(writer));
    outcome
}

/// How the probe went, as the success line writes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Probe {
    Passed(u16),
    Skipped,
    NotRejected(u16),
}

impl fmt::Display for Probe {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Passed(status) => write!(formatter, "probe passed http {status}"),
            Self::Skipped => formatter.write_str("probe skipped http -"),
            Self::NotRejected(status) => write!(formatter, "probe not-rejected http {status}"),
        }
    }
}

/// The endpoint `target` names, sent `key` as its entry declares.
fn endpoint(target: &CredentialTarget, key: &Key) -> Result<HostedEndpoint, CredentialFailure> {
    let invalid = |detail: String| {
        Refusal::new(
            StartupRefusal::ConfigValue,
            format!("{} {detail}", target.key),
        )
    };
    let bearer = TargetBearer::new(key.0.clone()).map_err(|_| {
        CredentialFailure::new(
            CredentialRefusal::InputNotAToken,
            "the key must be one printable ASCII token",
        )
    })?;
    let credential = match (target.auth_kind, &target.credential_header) {
        (AuthKind::Bearer, _) => EndpointCredential::Bearer(bearer),
        (AuthKind::ApiKey, Some(header)) => EndpointCredential::ApiKey {
            header: header.clone(),
            key: bearer,
        },
        _ => return Err(invalid("declares no credential header".to_owned()).into()),
    };
    Ok(HostedEndpoint::new(&target.base_url, credential)
        .map_err(|error| invalid(format!("base_url: {error}")))?
        .with_headers(target.headers.clone())
        .map_err(|error| invalid(format!("headers {error}")))?)
}

/// Probes `target`'s endpoint with `key` over `tls`.
fn probe(
    target: &CredentialTarget,
    key: &Key,
    tls: &dyn PodConnector,
) -> Result<Probe, CredentialFailure> {
    let endpoint = endpoint(target, key)?;
    let status = endpoint.probe(tls).map_err(|error| {
        CredentialFailure::new(
            CredentialRefusal::ProbeUnreachable,
            format!(
                "the probe of {} failed: {error}; nothing was written (--no-probe stores \
                 without it)",
                endpoint.authority()
            ),
        )
    })?;
    match status {
        401 | 403 => Err(CredentialFailure::new(
            CredentialRefusal::ProbeRejected,
            format!(
                "{} answered the probe {status}: the key was rejected; nothing was written",
                endpoint.authority()
            ),
        )),
        200..=299 => Ok(Probe::Passed(status)),
        _ => Ok(Probe::NotRejected(status)),
    }
}

/// The directory `target.path` lives in: it must exist, and be owned by this user or root and
/// not group- or world-writable, so that no other user can replace the file the server reads.
/// A refusal is `<setting>: <detail>`, naming the setting the path was read from and never the
/// path or its directory: a key written into the setting by mistake is not printed.
fn directory(target: &CredentialTarget) -> Result<PathBuf, CredentialFailure> {
    let setting = &target.key;
    let dir = match target.path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let missing = |detail: &dyn fmt::Display| {
        CredentialFailure::new(
            CredentialRefusal::DirectoryMissing,
            format!("{setting}: its directory {detail}"),
        )
    };
    let metadata =
        fs::metadata(&dir).map_err(|error| missing(&format_args!("cannot be read: {error}")))?;
    if !metadata.is_dir() {
        return Err(missing(&"is not a directory"));
    }
    let owner = metadata.uid();
    let mode = metadata.permissions().mode() & 0o7777;
    if owner != geteuid().as_raw() && owner != 0 {
        return Err(CredentialFailure::new(
            CredentialRefusal::DirectoryUnsafe,
            format!("{setting}: its directory is owned by neither this user nor root"),
        ));
    }
    if mode & 0o022 != 0 {
        return Err(CredentialFailure::new(
            CredentialRefusal::DirectoryUnsafe,
            format!(
                "{setting}: its directory has mode {mode:04o}: another user could replace the \
                 key file; none of 022 may be set"
            ),
        ));
    }
    Ok(dir)
}

/// Writes `key` to a temporary file in `dir`, reads it back through the server's reader and
/// token rule, and renames it over `target.path`. The temporary file is gone on every path. A
/// refusal names `target.key`, the setting, and never the path.
fn write(target: &CredentialTarget, dir: &Path, key: &Key) -> Result<(), CredentialFailure> {
    let failed = |step: &str, detail: &dyn fmt::Display| {
        CredentialFailure::new(
            CredentialRefusal::Write,
            format!("{}: {step}: {detail}", target.key),
        )
    };
    let name = target
        .path
        .file_name()
        .ok_or_else(|| failed("names no file", &"a key file is a path to a file"))?;
    let mut temporary_name = OsString::from(".");
    temporary_name.push(name);
    temporary_name.push(format!(".{}.tmp", std::process::id()));
    let temporary = dir.join(temporary_name);
    let flags = OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let descriptor = open(&temporary, flags, Mode::from_raw_mode(0o600))
        .map_err(|errno| failed("creating the temporary file", &errno))?;
    let outcome = (|| {
        let mut file = File::from(descriptor);
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|error| failed("setting mode 0600", &error))?;
        file.write_all(&key.0)
            .and_then(|()| file.sync_all())
            .map_err(|error| failed("writing the temporary file", &error))?;
        drop(file);
        let read = Key(read_token(
            &temporary,
            &target.key,
            &trusted::ENDPOINT_CREDENTIAL,
            (
                StartupRefusal::EndpointCredentialTooLarge,
                StartupRefusal::EndpointCredentialNotAToken,
            ),
        )?);
        if read.0 != key.0 {
            return Err(failed(
                "reading the temporary file back",
                &"the reader read other bytes than were written",
            ));
        }
        fs::rename(&temporary, &target.path).map_err(|error| failed("renaming over it", &error))?;
        // The rename is durable once the directory is: best effort, the key is already in place.
        if let Ok(directory) = File::open(dir) {
            drop(directory.sync_all());
        }
        Ok(())
    })();
    if outcome.is_err() {
        drop(fs::remove_file(&temporary));
    }
    outcome
}

/// `credential set`: the target, the key, the probe, the write. Returns the success line's text
/// after `credential stored `.
fn set(args: &SetArgs, tls: &dyn PodConnector) -> Result<String, CredentialFailure> {
    let deployment = load(&args.config)?;
    let (kind, name) = match (&args.model, &args.route) {
        (Some(model), None) => (TargetKind::Model, model),
        (None, Some(route)) => (TargetKind::Route, route),
        // clap's group admits exactly one.
        _ => unreachable!("exactly one of --model and --route"),
    };
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let target = target(&deployment, kind, name, home.as_deref())?;
    let key = if args.stdin {
        read_stdin()?
    } else {
        read_terminal(&target)?
    };
    let dir = directory(&target)?;
    let probed = if args.no_probe {
        Probe::Skipped
    } else {
        probe(&target, &key, tls)?
    };
    write(&target, &dir, &key)?;
    // The setting, never the path it holds: a key written there by mistake is not printed.
    Ok(format!("{} {probed}", target.key))
}

fn report(line: fmt::Arguments<'_>) {
    drop(writeln!(io::stderr(), "b10x-llm-gateway: {line}"));
}

/// The `credential` command line, `args` starting at `credential`; `tls` reaches every `https`
/// endpoint, as it does for the server.
pub(crate) fn main(args: Vec<OsString>, tls: &dyn PodConnector) -> ExitCode {
    use clap::Parser as _;
    let cli = match CredentialCli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(error) => {
            use clap::error::ErrorKind;
            if matches!(
                error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) {
                error.exit();
            }
            // clap quotes the offending argument, which may be a key: name only the kind.
            drop(writeln!(
                io::stderr(),
                "b10x-llm-gateway: usage error ({:?}); see `b10x-llm-gateway credential set --help`",
                error.kind()
            ));
            return ExitCode::from(2);
        }
    };
    let CredentialCommand::Set(set_args) = cli.command;
    match set(&set_args, tls) {
        Ok(stored) => {
            report(format_args!("credential stored {stored}"));
            ExitCode::SUCCESS
        }
        Err(failure) => {
            report(format_args!("refused {failure}"));
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_KEY_BYTES, check_key, read_hidden_line};

    #[test]
    fn a_hang_up_before_the_newline_cancels_and_never_takes_the_partial_line() {
        let cancelled = |input: &[u8]| read_hidden_line(&mut &*input).map(|key| key.0.clone());
        assert_eq!(cancelled(b"sk-1\n").unwrap(), b"sk-1");
        assert_eq!(cancelled(b"sk-1\r").unwrap(), b"sk-1");
        for input in [&b"sk-1"[..], b"", b"sk-1\x7f"] {
            assert_eq!(
                cancelled(input).unwrap_err().code(),
                "credential:input-cancelled",
                "{input:?}"
            );
        }
        assert_eq!(
            cancelled(b"sk-1\x03").unwrap_err().code(),
            "credential:input-cancelled"
        );
        assert_eq!(
            cancelled(b"sk-1\x04").unwrap_err().code(),
            "credential:input-cancelled"
        );
    }

    #[test]
    fn a_key_is_one_printable_token_up_to_one_final_newline() {
        assert_eq!(check_key(b"sk-1\n").unwrap(), b"sk-1");
        assert_eq!(check_key(b"sk-1").unwrap(), b"sk-1");
        let longest = vec![b'k'; MAX_KEY_BYTES];
        assert_eq!(check_key(&longest).unwrap(), longest);
        for (input, code) in [
            (&b""[..], "credential:input-empty"),
            (b"\n", "credential:input-empty"),
            (b"sk-1\r\n", "credential:input-not-a-token"),
            (b"sk-1\n\n", "credential:input-not-a-token"),
            (b" sk-1", "credential:input-not-a-token"),
            (b"sk 1", "credential:input-not-a-token"),
        ] {
            assert_eq!(check_key(input).unwrap_err().code(), code, "{input:?}");
        }
        let over = vec![b'k'; MAX_KEY_BYTES + 1];
        assert_eq!(
            check_key(&over).unwrap_err().code(),
            "credential:input-too-large"
        );
    }

    /// What this proves, through the debug assertion behind every typed byte: a key typed up to
    /// the bound and past it, with Backspace and Ctrl-U, never outgrows the allocation the
    /// prompt took before its first byte, so no copy of it is left in freed memory by a
    /// reallocation. The line is a `Key`, overwritten when dropped.
    #[test]
    fn a_typed_key_never_outgrows_the_allocation_taken_before_its_first_byte() {
        let typed = |input: &[u8]| read_hidden_line(&mut &*input).map(|key| key.0.len());
        let mut longest = vec![b'k'; MAX_KEY_BYTES + 1];
        longest.extend_from_slice(b"\x7f\n");
        assert_eq!(typed(&longest).unwrap(), MAX_KEY_BYTES);
        let mut cleared = vec![b'k'; MAX_KEY_BYTES];
        cleared.push(0x15);
        cleared.extend_from_slice(&vec![b'k'; MAX_KEY_BYTES]);
        cleared.push(b'\r');
        assert_eq!(typed(&cleared).unwrap(), MAX_KEY_BYTES);
        let mut over = vec![b'k'; MAX_KEY_BYTES + 3];
        over.push(b'\n');
        assert_eq!(
            typed(&over).unwrap_err().code(),
            "credential:input-too-large"
        );
    }
}
