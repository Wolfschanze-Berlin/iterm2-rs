use arboard::Clipboard;

pub struct ClipboardManager {
    clipboard: Clipboard,
}

impl ClipboardManager {
    pub fn new() -> anyhow::Result<Self> {
        let clipboard = Clipboard::new()?;
        Ok(Self { clipboard })
    }

    pub fn copy(&mut self, text: &str) -> anyhow::Result<()> {
        self.clipboard.set_text(text)?;
        Ok(())
    }

    pub fn paste(&mut self) -> anyhow::Result<String> {
        let text = self.clipboard.get_text()?;
        Ok(text)
    }
}
