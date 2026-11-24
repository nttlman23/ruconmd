//! Консольный редактор Markdown.
//!
//! `key`, `keymap`, `input`, `app` не обращаются к терминалу и тестируются отдельно;
//! работа с терминалом собрана в `term`, отрисовка в `ui`.

pub mod app;
pub mod buffer;
pub mod cli;
pub mod clipboard;
pub mod command;
pub mod diag;
pub mod dialog;
pub mod encoding;
pub mod file;
pub mod i18n;
pub mod input;
pub mod key;
pub mod keymap;
pub mod layout;
pub mod term;
pub mod ui;
pub mod view;
