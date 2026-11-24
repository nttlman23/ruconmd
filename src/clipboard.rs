//! Буфер обмена: системный, а при его недоступности внутренний.

use crate::buffer::Eol;

pub trait Clipboard {
    fn set(&mut self, text: &str);
    fn get(&mut self) -> Option<String>;
}

/// Внутренний буфер.
#[derive(Default)]
pub struct Local(Option<String>);

impl Clipboard for Local {
    fn set(&mut self, text: &str) {
        self.0 = Some(text.to_owned());
    }

    fn get(&mut self) -> Option<String> {
        self.0.clone()
    }
}

/// Подключается при первом обращении: без графического сеанса это может не выйти.
#[derive(Default)]
pub struct System {
    system: Option<arboard::Clipboard>,
    local: Local,
    tried: bool,
    /// Последнее копирование не дошло до системного буфера.
    local_is_newer: bool,
}

impl System {
    fn system(&mut self) -> Option<&mut arboard::Clipboard> {
        if !self.tried {
            self.tried = true;
            self.system = arboard::Clipboard::new().ok();
        }
        self.system.as_mut()
    }
}

impl Clipboard for System {
    fn set(&mut self, text: &str) {
        let unix = text.replace("\r\n", "\n");
        let native = match Eol::PLATFORM {
            Eol::Lf => unix.clone(),
            Eol::Crlf => unix.replace('\n', "\r\n"),
        };
        let stored = self.system().is_some_and(|c| c.set_text(native).is_ok());
        self.local_is_newer = !stored;
        self.local.set(&unix);
    }

    fn get(&mut self) -> Option<String> {
        if self.local_is_newer {
            return self.local.get();
        }
        match self.system()?.get_text() {
            Ok(text) => Some(text),
            Err(arboard::Error::ContentNotAvailable) => None,
            Err(_) => self.local.get(),
        }
    }
}
