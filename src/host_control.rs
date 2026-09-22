//! Runtime commands run in the signed, long-lived application. A bundled CLI
//! has neither the app's Network Extension entitlements nor its live session.
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::catalog::Catalog;
use crate::backend;
use crate::network_extension::{self, DnsPhase, Phase, RuntimeStatus};
use crate::strategy::Strategy;
use crate::supervisor::{OperationState, RuntimeIdentity, Supervisor};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Request {
    Status,
    Apply { refresh: bool },
    Connect,
    Disconnect,
    Select { group: String, name: String },
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AppliedCatalog {
    pub nodes: usize,
    pub excluded: usize,
    pub filter_excluded: usize,
    pub fetch_failures: usize,
    pub refresh_warnings: Vec<String>,
}

impl From<&Catalog> for AppliedCatalog {
    fn from(catalog: &Catalog) -> Self {
        Self {
            nodes: catalog.nodes.len(),
            excluded: catalog.excluded.len(),
            filter_excluded: catalog.filter_excluded_count(),
            fetch_failures: catalog.fetch_failure_count(),
            refresh_warnings: catalog.refresh_warnings(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Snapshot {
    pub operation: OperationState,
    pub runtime: Option<RuntimeIdentity>,
    pub controller_ready: bool,
    pub extension: RuntimeStatus,
    pub extension_required: bool,
    pub catalog: Option<AppliedCatalog>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub xray: Option<crate::xray::XrayStatus>,
}

/// Bundled macOS clients must never fall back to executing NE calls themselves.
pub fn request(request: Request) -> Result<Snapshot> {
    if backend::is_xray() && !crate::login_item::is_bundled() && !matches!(request,Request::Status) {
        bail!("请使用 MyProxy.app 内的命令行工具，运行连接由应用持有");
    }
    #[cfg(target_os = "macos")]
    {
        if crate::login_item::is_bundled() {
            return transport::request(request);
        }
        // An unbundled developer CLI cannot prove that the signed app released
        // DNS. Its synthetic Unbundled status must not authorize killing that core.
        let uses_extension = Strategy::load().is_ok_and(|strategy| strategy.system_extension)
            || Supervisor::shared()
                .applied_strategy()
                .is_some_and(|strategy| strategy.system_extension);
        if uses_extension {
            if matches!(request, Request::Status) {
                return Ok(snapshot(&Supervisor::shared(), host_unavailable()));
            }
            bail!("System Extension control requires the myproxyctl bundled with the signed app");
        }
    }
    execute(request)
}

#[cfg(target_os = "macos")]
fn host_unavailable() -> RuntimeStatus {
    RuntimeStatus {
        phase: Phase::Disabled,
        dns_phase: DnsPhase::Unknown,
        observed: false,
        desired_revision: 0,
        applied_revision: None,
        message: Some("myproxy Host is unavailable; use the bundled CLI and open the updated app to read System Extension/DNS status".into()),
        dns_message: None,
        capture_enabled: false,
        fail_open: true,
    }
}

fn execute(request: Request) -> Result<Snapshot> {
    let strategy = if matches!(request, Request::Disconnect) {
        Strategy::load().unwrap_or_default()
    } else {
        Strategy::load()?
    };
    let supervisor = Supervisor::shared();
    supervisor.adopt_running(strategy.tun, strategy.system_extension, strategy.mixed_port);
    let catalog = match &request {
        Request::Status => None,
        Request::Select { group, name } => {
            if !backend::is_xray() { bail!("此操作仅适用于 Xray 通道"); }
            let identity = supervisor.runtime_identity().context("尚未连接")?;
            supervisor.select_proxy(identity, group, name)?;
            None
        },
        Request::Apply { refresh } => Some(AppliedCatalog::from(&if *refresh {
            supervisor.apply(&strategy)?
        } else {
            supervisor.apply_cached(&strategy)?
        })),
        Request::Connect => {
            supervisor.connect(&strategy)?;
            None
        }
        Request::Disconnect => {
            supervisor.disconnect()?;
            None
        }
    };
    let extension = if matches!(request, Request::Status) {
        network_extension::status()
    } else {
        // The app owns approval and activation after the client exits. Do not
        // cancel a submitted operation just because this reply reaches a deadline.
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let status = network_extension::status();
            let pending = !status.observed
                || status.is_pending()
                || (status.phase == Phase::Running
                    && matches!(status.dns_phase, DnsPhase::Waiting | DnsPhase::Unknown));
            if !pending || Instant::now() >= deadline {
                break status;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    };
    let mut snapshot = snapshot(&supervisor, extension);
    snapshot.catalog = catalog;
    if !matches!(request, Request::Status)
        && (snapshot.operation.is_busy()
            || (matches!(request, Request::Connect) && snapshot.runtime.is_none())
            || (matches!(request, Request::Disconnect) && snapshot.runtime.is_some()))
    {
        bail!("Runtime changed during the command; check status before retrying");
    }
    Ok(snapshot)
}

fn snapshot(supervisor: &Supervisor, extension: RuntimeStatus) -> Snapshot {
    let runtime = supervisor.runtime_identity();
    Snapshot {
        operation: supervisor.operation_state(),
        controller_ready: backend::load().unwrap_or_default() == backend::BackendKind::Mihomo
            && runtime
                .is_some_and(|identity| crate::controller::ready(identity.mixed_port).is_ok()),
        extension_required: runtime.is_some()
            && supervisor
                .applied_strategy()
                .is_some_and(|strategy| strategy.system_extension),
        runtime,
        extension,
        catalog: None,
        xray: if backend::is_xray() { crate::xray::status().ok() } else { None },
    }
}

pub fn check_outcome(snapshot: &Snapshot) -> Result<()> {
    if let Some(xray) = &snapshot.xray {
        if xray.wanted && !xray.ready && !snapshot.extension_required { bail!("Xray 入口未就绪"); }
        if !snapshot.extension_required { return Ok(()); }
    }
    let status = &snapshot.extension;
    if !status.observed {
        bail!("System Extension/DNS status is not confirmed; check status in myproxy");
    }
    if status.phase == Phase::Failed || status.dns_phase == DnsPhase::Failed {
        bail!(
            "System Extension/DNS failed: {}",
            status
                .message
                .as_deref()
                .or(status.dns_message.as_deref())
                .unwrap_or("check status in myproxy")
        );
    }
    if snapshot.extension_required
        && (status.phase != Phase::Running || status.dns_phase != DnsPhase::Running)
    {
        bail!("System Extension/DNS is not ready; complete setup or check status in myproxy");
    }
    Ok(())
}

/// Called once by the GUI after acquiring its instance lock and starting its
/// event loop. Runtime work stays off the GUI thread.
pub fn start() -> Result<()> {
    #[cfg(target_os = "macos")]
    if crate::login_item::is_bundled() {
        transport::start()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_ready_does_not_hide_unconfirmed_capture_or_dns() {
        let mut snapshot = Snapshot {
            operation: OperationState::Connected,
            runtime: Some(RuntimeIdentity {
                generation: 1,
                mixed_port: 7890,
            }),
            controller_ready: true,
            extension_required: true,
            catalog: None,
            xray: None,
            extension: RuntimeStatus {
                phase: Phase::Running,
                dns_phase: DnsPhase::Running,
                observed: true,
                desired_revision: 1,
                applied_revision: Some(1),
                message: None,
                dns_message: None,
                capture_enabled: true,
                fail_open: true,
            },
        };
        assert!(check_outcome(&snapshot).is_ok());
        for phase in [DnsPhase::Unknown, DnsPhase::Waiting, DnsPhase::Failed] {
            snapshot.extension.dns_phase = phase;
            assert!(check_outcome(&snapshot).is_err());
        }
        snapshot.extension.dns_phase = DnsPhase::Running;
        snapshot.extension.phase = Phase::WaitingApproval;
        assert!(check_outcome(&snapshot).is_err());
        snapshot.extension.phase = Phase::Running;
        snapshot.extension.observed = false;
        assert!(check_outcome(&snapshot).is_err());
    }
}

#[cfg(target_os = "macos")]
mod transport {
    use super::*;
    use std::fs;
    use std::io::{Read, Write};
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::PathBuf;

    const PROTOCOL: u32 = 1;
    const MAX_FRAME: usize = 256 * 1024;

    #[derive(Debug, Serialize, Deserialize)]
    struct Envelope {
        protocol: u32,
        bundle: PathBuf,
        request: Request,
    }

    fn bundle() -> Result<PathBuf> {
        crate::login_item::app_bundle_path()
            .context("Runtime control requires the installed myproxy app")?
            .canonicalize()
            .context("locate installed myproxy app")
    }

    fn socket_path() -> Result<PathBuf> {
        let directory = crate::paths::data_dir()?.join("control");
        match fs::DirBuilder::new().mode(0o700).create(&directory) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error).context("create runtime control directory"),
        }
        let metadata = fs::symlink_metadata(&directory)?;
        if !metadata.is_dir()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.permissions().mode() & 0o077 != 0
        {
            bail!("Runtime control directory must be owned by this user with mode 0700");
        }
        Ok(directory.join("host.sock"))
    }

    fn peer_is_current_user(stream: &UnixStream) -> Result<()> {
        let mut uid = 0;
        let mut gid = 0;
        let result = unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) };
        if result != 0 || uid != unsafe { libc::geteuid() } {
            bail!("Runtime control peer does not belong to this user");
        }
        Ok(())
    }

    fn read_frame<T: serde::de::DeserializeOwned>(stream: &mut UnixStream) -> Result<T> {
        let mut header = [0; 4];
        stream.read_exact(&mut header)?;
        let length = u32::from_be_bytes(header) as usize;
        if length > MAX_FRAME {
            bail!("Runtime control message is too large");
        }
        let mut bytes = vec![0; length];
        stream.read_exact(&mut bytes)?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    fn write_frame<T: Serialize>(stream: &mut UnixStream, value: &T) -> Result<()> {
        let bytes = serde_json::to_vec(value)?;
        if bytes.len() > MAX_FRAME {
            bail!("Runtime control message is too large");
        }
        stream.write_all(&(bytes.len() as u32).to_be_bytes())?;
        stream.write_all(&bytes)?;
        Ok(())
    }

    pub(super) fn request(request: Request) -> Result<Snapshot> {
        let path = socket_path()?;
        let mut connection = UnixStream::connect(&path);
        if connection.is_err() && matches!(request, Request::Status) {
            return Ok(snapshot(&Supervisor::shared(), host_unavailable()));
        }
        if connection.is_err() {
            let mut launch = std::process::Command::new("/usr/bin/open");
            launch.arg("-g").arg("-a").arg(bundle()?);
            if let Some(directory) = std::env::var_os(crate::paths::data_dir_env()) {
                let mut assignment = std::ffi::OsString::from(crate::paths::data_dir_env());
                assignment.push("=");
                assignment.push(directory);
                launch.arg("--env").arg(assignment);
            }
            if !launch
                .args(["--args", "--host-control"])
                .status()?
                .success()
            {
                bail!("Could not open myproxy; open the installed app and retry");
            }
            let deadline = Instant::now() + Duration::from_secs(10);
            while connection.is_err() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(100));
                connection = UnixStream::connect(&path);
            }
        }
        let mut stream =
            connection.context("myproxy Host is unavailable; open the updated app and retry")?;
        peer_is_current_user(&stream)?;
        stream.set_read_timeout(Some(Duration::from_secs(120)))?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
        write_frame(
            &mut stream,
            &Envelope {
                protocol: PROTOCOL,
                bundle: bundle()?,
                request,
            },
        )?;
        // Never replay a mutation after a lost reply: the Host may have applied it.
        let response: std::result::Result<Snapshot, String> = read_frame(&mut stream)
            .context("Host reply was not received; the operation may still be running; use status before retrying")?;
        response.map_err(anyhow::Error::msg)
    }

    pub(super) fn start() -> Result<()> {
        let path = socket_path()?;
        match fs::symlink_metadata(&path) {
            Ok(metadata)
                if metadata.file_type().is_socket()
                    && metadata.uid() == unsafe { libc::geteuid() } =>
            {
                fs::remove_file(&path)?;
            }
            Ok(_) => bail!("Runtime control socket path is occupied by another file"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let listener = UnixListener::bind(&path)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        let bundle = bundle()?;
        std::thread::Builder::new()
            .name("host-control".into())
            .spawn(move || {
                for connection in listener.incoming() {
                    let Ok(stream) = connection else { continue };
                    let bundle = bundle.clone();
                    // Status and cancellation must remain available during connect.
                    if let Err(error) = std::thread::Builder::new()
                        .name("host-command".into())
                        .spawn(move || {
                            if let Err(error) = serve(stream, &bundle, execute) {
                                crate::log::warn("host-control", format!("{error:#}"));
                            }
                        })
                    {
                        crate::log::error("host-control", format!("start command worker: {error}"));
                    }
                }
            })?;
        Ok(())
    }

    fn serve(
        mut stream: UnixStream,
        bundle: &std::path::Path,
        handler: impl FnOnce(Request) -> Result<Snapshot>,
    ) -> Result<()> {
        peer_is_current_user(&stream)?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
        let response = (|| {
            let envelope: Envelope = read_frame(&mut stream)?;
            if envelope.protocol != PROTOCOL || envelope.bundle != bundle {
                bail!(
                    "Runtime control version or app differs; reopen the app that supplied this CLI"
                );
            }
            handler(envelope.request)
        })()
        .map_err(|error| format!("{error:#}"));
        write_frame(&mut stream, &response)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn wrong_bundle_does_not_execute_a_command() {
            let (mut client, server) = UnixStream::pair().unwrap();
            let worker = std::thread::spawn(move || {
                serve(server, std::path::Path::new("/right.app"), |_| {
                    panic!("must not execute")
                })
            });
            write_frame(
                &mut client,
                &Envelope {
                    protocol: PROTOCOL,
                    bundle: "/wrong.app".into(),
                    request: Request::Disconnect,
                },
            )
            .unwrap();
            let response: std::result::Result<Snapshot, String> = read_frame(&mut client).unwrap();
            assert!(response.unwrap_err().contains("app differs"));
            worker.join().unwrap().unwrap();
        }

        #[test]
        fn command_failure_is_returned_without_replay() {
            let (mut client, server) = UnixStream::pair().unwrap();
            let worker = std::thread::spawn(move || {
                serve(server, std::path::Path::new("/myproxy.app"), |request| {
                    assert!(matches!(request, Request::Apply { refresh: true }));
                    bail!("DNS disable unconfirmed; core retained")
                })
            });
            write_frame(
                &mut client,
                &Envelope {
                    protocol: PROTOCOL,
                    bundle: "/myproxy.app".into(),
                    request: Request::Apply { refresh: true },
                },
            )
            .unwrap();
            let response: std::result::Result<Snapshot, String> = read_frame(&mut client).unwrap();
            assert_eq!(
                response.unwrap_err(),
                "DNS disable unconfirmed; core retained"
            );
            worker.join().unwrap().unwrap();
        }

        #[test]
        fn oversized_frame_is_rejected_before_reading_the_body() {
            let (mut client, mut server) = UnixStream::pair().unwrap();
            client
                .write_all(&((MAX_FRAME + 1) as u32).to_be_bytes())
                .unwrap();
            assert!(read_frame::<Envelope>(&mut server)
                .unwrap_err()
                .to_string()
                .contains("too large"));
        }
    }
}
