use coincubed::config::BitcoinBackend;

pub mod bitcoind;
pub mod electrum;
pub mod esplora;
pub mod revalidate;
pub mod tor;

/// Configure `command` so its spawned child is detached from this process:
/// closing Coincube must not take the managed node or Tor down with it (both are
/// stopped explicitly instead). On Windows via `CREATE_NO_WINDOW |
/// DETACHED_PROCESS`; on Unix by starting a new session (`setsid`) from a
/// `pre_exec` hook so an app exit doesn't `SIGHUP` the child. Shared by the
/// bitcoind and Tor spawn paths so the detach policy can't drift between them.
pub(crate) fn detach_spawned_process(command: &mut std::process::Command) {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        const DETACHED_PROCESS: u32 = 0x00000008;
        command.creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: the closure only calls the async-signal-safe `setsid()` and
        // captures no state, so it's safe to run between fork and exec.
        unsafe {
            command.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Copy)]
pub enum NodeType {
    Bitcoind,
    Electrum,
    Esplora,
}

impl From<&BitcoinBackend> for NodeType {
    fn from(bitcoin_backend: &BitcoinBackend) -> Self {
        match bitcoin_backend {
            BitcoinBackend::Bitcoind(_) => Self::Bitcoind,
            BitcoinBackend::Electrum(_) => Self::Electrum,
            BitcoinBackend::Esplora(_) => Self::Esplora,
        }
    }
}
