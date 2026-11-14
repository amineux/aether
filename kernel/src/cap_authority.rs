//! Boot-designated address-space authority for the single bootstrap cap table.
//! Cloned threads share a root. Other images never inherit this table implicitly.
#[derive(Default)]
pub struct CapAuthority {
    root: Option<u64>,
}

impl CapAuthority {
    pub const fn new() -> Self {
        Self { root: None }
    }

    /// Called only by trusted boot code, before entering userspace. Never rebind.
    pub fn bind(&mut self, root: u64) -> Result<(), ()> {
        if root == 0 || self.root.is_some() {
            return Err(());
        }
        self.root = Some(root);
        Ok(())
    }

    pub fn permits(&self, caller_root: Option<u64>) -> bool {
        self.root.is_some() && self.root == caller_root
    }
}
