//! Native-only Apple backend; not registered in Setup or Run.
use super::{
    apple::{AppleHelperOwner, HelperError},
    apple_channel::GuestSlot,
    apple_input::{configuration_file, helper_path},
    image_types::{ImageTarget, LaunchMode},
    types::*,
};
use bridge_runtime_protocol::{Uuid, STATE_BYTES};
use std::{
    fs::{self, File, OpenOptions},
    os::unix::fs::OpenOptionsExt,
    path::PathBuf,
    process::Command,
    sync::Arc,
    time::{Duration, Instant},
};
pub struct AppleBackend {
    resource_root: PathBuf,
    owner: Option<AppleHelperOwner>,
    ownership: Option<Arc<File>>,
    create_efi: bool,
    current: Option<AppleBackendHandle>,
    poisoned: bool,
}
pub(crate) struct AppleBackendHandle {
    id: Uuid,
    pub(crate) guest: GuestSlot,
}
impl AppleBackend {
    pub fn new(resource_root: PathBuf) -> Self {
        Self {
            resource_root,
            owner: None,
            ownership: None,
            create_efi: false,
            current: None,
            poisoned: false,
        }
    }
    fn validate(&self, spec: &LaunchSpec) -> Result<(), RuntimeError> {
        if self.poisoned {
            return Err(RuntimeError::RetainedState);
        }
        if (!cfg!(all(target_os = "macos", target_arch = "aarch64")) && !cfg!(test))
            || spec.backend != BackendKind::Apple
            || spec.image.target() != ImageTarget::MacosArm64
            || spec.image.launch_mode() != LaunchMode::Uefi
            || spec.image.os_format() != "raw"
        {
            return Err(RuntimeError::Unavailable);
        }
        if spec.state_bytes != STATE_BYTES
            || spec.state_path != spec.root.join("state.disk")
            || self
                .ownership
                .as_ref()
                .is_some_and(|owner| !Arc::ptr_eq(owner, &spec.ownership))
        {
            return Err(RuntimeError::RetainedState);
        }
        Ok(())
    }
    fn selected(&self, handle: &BackendHandle) -> Result<&AppleBackendHandle, RuntimeError> {
        let handle = handle
            .get::<AppleBackendHandle>()
            .ok_or(RuntimeError::Unavailable)?;
        if self
            .current
            .as_ref()
            .is_none_or(|current| current.id != handle.id)
        {
            return Err(RuntimeError::Unavailable);
        }
        Ok(self.current.as_ref().unwrap())
    }
}
fn helper_error(error: HelperError) -> RuntimeError {
    match error {
        HelperError::Busy => RuntimeError::Busy,
        HelperError::ExitUnobserved => {
            RuntimeError::StopUnconfirmed("Apple helper exit unobserved".into())
        }
        HelperError::Io(kind) => RuntimeError::Io(kind.into()),
        _ => RuntimeError::RetainedState,
    }
}
impl VmBackend for AppleBackend {
    fn create_state(&mut self, spec: &LaunchSpec) -> Result<(), RuntimeError> {
        self.validate(spec)?;
        if self.ownership.is_some() || fs::symlink_metadata(spec.root.join("efi.vars")).is_ok() {
            return Err(RuntimeError::RetainedState);
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&spec.state_path)?;
        if !file.metadata()?.is_file() {
            return Err(RuntimeError::RetainedState);
        }
        file.set_len(STATE_BYTES)?;
        file.sync_all()?;
        self.ownership = Some(spec.ownership.clone());
        self.create_efi = true;
        Ok(())
    }
    fn start(&mut self, spec: &LaunchSpec) -> Result<BackendHandle, RuntimeError> {
        self.validate(spec)?;
        if self.current.is_some() {
            return Err(RuntimeError::Busy);
        }
        if self.owner.is_none() {
            self.ownership = Some(spec.ownership.clone());
            self.owner = Some(AppleHelperOwner::new(spec.ownership.clone()));
        }
        // The one live preparation permission is consumed before ANY launch attempt.
        let create_efi = std::mem::take(&mut self.create_efi);
        let efi = spec.root.join("efi.vars");
        if create_efi {
            if fs::symlink_metadata(&efi).is_ok() {
                return Err(RuntimeError::RetainedState);
            }
        } else {
            let metadata = fs::symlink_metadata(&efi)?;
            if !metadata.is_file() || metadata.len() == 0 {
                return Err(RuntimeError::RetainedState);
            }
        }
        let file = configuration_file(spec, create_efi)?;
        let socket = self
            .owner
            .as_mut()
            .unwrap()
            .start(Command::new(helper_path(&self.resource_root)?), file)
            .map_err(helper_error)?;
        let guest = GuestSlot::new(socket);
        let id = Uuid::new_v4();
        self.current = Some(AppleBackendHandle {
            id,
            guest: guest.clone(),
        });
        Ok(BackendHandle::new(AppleBackendHandle { id, guest }))
    }
    fn stop(&mut self, handle: &BackendHandle) -> Result<(), RuntimeError> {
        let guest = self.selected(handle)?.guest.clone();
        guest.seal();
        let stopped = self.owner.as_mut().unwrap().stop().map_err(helper_error);
        let revoked = guest
            .revoke(Instant::now() + Duration::from_secs(10))
            .map_err(RuntimeError::Io);
        if stopped.is_err() || revoked.is_err() {
            self.poisoned = true;
        }
        stopped?;
        revoked?;
        self.current = None;
        Ok(())
    }
    fn pause(&mut self, handle: &BackendHandle) -> Result<(), RuntimeError> {
        self.selected(handle)?;
        if self.poisoned {
            return Err(RuntimeError::RetainedState);
        }
        match self.owner.as_mut().unwrap().pause().map_err(helper_error) {
            Ok(()) => Ok(()),
            Err(error) => {
                let _ = self.stop(handle); // Revoke even when poisoned cleanup cannot confirm exit.
                Err(error)
            }
        }
    }
    fn resume(&mut self, handle: &BackendHandle) -> Result<(), RuntimeError> {
        self.selected(handle)?;
        if self.poisoned {
            return Err(RuntimeError::RetainedState);
        }
        match self.owner.as_mut().unwrap().resume().map_err(helper_error) {
            Ok(()) => Ok(()),
            Err(error) => {
                let _ = self.stop(handle);
                Err(error)
            }
        }
    }
}
#[cfg(test)]
#[path = "apple_backend_tests.rs"]
pub(crate) mod tests;
