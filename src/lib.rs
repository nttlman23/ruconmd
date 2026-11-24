//! Консольный редактор Markdown.
//!
//! `key`, `keymap`, `input`, `app` не обращаются к терминалу и тестируются отдельно;
//! работа с терминалом собрана в `term`, отрисовка в `ui`.

pub mod app;
pub mod buffer;
pub mod cli;
pub mod clipboard;
pub mod code;
pub mod command;
pub mod config;
pub mod diag;
pub mod dialog;
pub mod encoding;
pub mod file;
pub mod files;
pub mod graphics;
pub mod i18n;
pub mod input;
pub mod key;
pub mod keymap;
pub mod layout;
pub mod links;
pub mod mdline;
pub mod pane;
pub mod preview;
pub mod recovery;
pub mod search;
pub mod settings;
pub mod style;
pub mod term;
pub mod tex;
pub mod typing;
pub mod ui;
pub mod update;
pub mod view;
