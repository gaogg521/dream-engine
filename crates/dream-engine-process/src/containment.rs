use std::io::{Error, Result};

use tokio::process::{Child, Command};

#[cfg(windows)]
use crate::windows_job::JobObject;

/// Owns a spawned process tree. Configure before spawn, attach before allowing
/// child work, and terminate before releasing the child, including cancellation.
pub struct ChildContainment {
    #[cfg(windows)]
    job: JobObject,
}

impl ChildContainment {
    /// Configure a private process group or suspended Windows process.
    pub fn configure(command: &mut Command) {
        configure_command(command);
    }

    /// Attach tree ownership and resume a suspended Windows child.
    pub fn attach(child: &mut Child) -> Result<Self> {
        attach_child(child, false)
    }

    /// Attach an MCP-owned tree which must not outlive its owner, including
    /// a Windows host crash. Detached command execution retains its semantics.
    pub fn attach_owned(child: &mut Child) -> Result<Self> {
        attach_child(child, true)
    }

    /// Terminate the owned tree, using the original PID even after leader exit.
    pub fn terminate(&self, child: &mut Child, child_id: Option<u32>) -> Result<()> {
        terminate_child(child, child_id, self)
    }
}

#[cfg(unix)]
fn configure_command(command: &mut Command) {
    command.process_group(0);
}

#[cfg(windows)]
fn configure_command(command: &mut Command) {
    use windows_sys::Win32::System::Threading::{CREATE_NO_WINDOW, CREATE_SUSPENDED};

    // CREATE_SUSPENDED: spawn paused so the job object can be attached before
    // the child runs (see `attach_child`).
    // CREATE_NO_WINDOW: don't flash a console window for every `ExecCommand`.
    // `creation_flags` replaces the whole set, so both must be passed here.
    command.creation_flags(CREATE_SUSPENDED | CREATE_NO_WINDOW);
}

#[cfg(not(any(unix, windows)))]
fn configure_command(_command: &mut Command) {}

#[cfg(windows)]
fn attach_child(child: &mut Child, kill_on_drop: bool) -> Result<ChildContainment> {
    let pid = child.id().ok_or_else(|| Error::other("spawned child has no pid"))?;
    let raw_handle = child
        .raw_handle()
        .ok_or_else(|| Error::other("spawned child has no raw handle"))?;

    let job = JobObject::assign_and_resume(raw_handle, pid, kill_on_drop)
        .map_err(|error| Error::other(format!("windows job containment failed: {error}")))?;

    Ok(ChildContainment { job })
}

#[cfg(not(windows))]
fn attach_child(_child: &mut Child, _kill_on_drop: bool) -> Result<ChildContainment> {
    Ok(ChildContainment {})
}

#[cfg(unix)]
fn terminate_child(child: &mut Child, child_id: Option<u32>, _containment: &ChildContainment) -> Result<()> {
    if let Some(target) = child_id.and_then(group_kill_target) {
        let rc = unsafe { libc::kill(target, libc::SIGKILL) };
        if rc == 0 {
            return Ok(());
        }

        let err = Error::last_os_error();
        if err.raw_os_error() == Some(libc::ESRCH) {
            return Ok(());
        }

        return Err(err);
    }

    child.start_kill()
}

#[cfg(windows)]
fn terminate_child(_child: &mut Child, _child_id: Option<u32>, containment: &ChildContainment) -> Result<()> {
    containment.job.terminate()
}

#[cfg(not(any(unix, windows)))]
fn terminate_child(child: &mut Child, _child_id: Option<u32>, _containment: &ChildContainment) -> Result<()> {
    child.start_kill()
}

#[cfg(unix)]
fn group_kill_target(pid: u32) -> Option<i32> {
    if pid <= 1 {
        return None;
    }

    i32::try_from(pid).ok().map(|pid| -pid)
}
