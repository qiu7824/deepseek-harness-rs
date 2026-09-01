use std::sync::Arc;

use crate::{core::Command, menu::MenuSpec};
use serde::{Deserialize, Serialize};

pub type TrayMenuProvider = Arc<dyn Fn() -> MenuSpec + Send + Sync + 'static>;

#[derive(Clone, Serialize, Deserialize)]
pub struct TraySpec {
    pub tooltip: Option<String>,
    pub icon_path: Option<String>,
    pub menu: MenuSpec,
    #[serde(skip)]
    pub menu_provider: Option<TrayMenuProvider>,
}

impl std::fmt::Debug for TraySpec {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TraySpec")
            .field("tooltip", &self.tooltip)
            .field("icon_path", &self.icon_path)
            .field("menu", &self.menu)
            .field("has_menu_provider", &self.menu_provider.is_some())
            .finish()
    }
}

impl PartialEq for TraySpec {
    fn eq(&self, other: &Self) -> bool {
        self.tooltip == other.tooltip
            && self.icon_path == other.icon_path
            && self.menu == other.menu
            && match (&self.menu_provider, &other.menu_provider) {
                (Some(left), Some(right)) => Arc::ptr_eq(left, right),
                (None, None) => true,
                _ => false,
            }
    }
}

impl Eq for TraySpec {}

impl TraySpec {
    pub fn new() -> Self {
        Self {
            tooltip: None,
            icon_path: None,
            menu: MenuSpec::new(),
            menu_provider: None,
        }
    }

    pub fn tooltip(mut self, tooltip: impl Into<String>) -> Self {
        self.tooltip = Some(tooltip.into());
        self
    }

    pub fn icon_path(mut self, icon_path: impl Into<String>) -> Self {
        self.icon_path = Some(icon_path.into());
        self
    }

    pub fn menu(mut self, menu: MenuSpec) -> Self {
        self.menu = menu;
        self
    }

    pub fn dynamic_menu(mut self, provider: impl Fn() -> MenuSpec + Send + Sync + 'static) -> Self {
        self.menu_provider = Some(Arc::new(provider));
        self
    }

    pub fn current_menu(&self) -> MenuSpec {
        self.menu_provider
            .as_ref()
            .map(|provider| provider())
            .unwrap_or_else(|| self.menu.clone())
    }

    pub fn item(mut self, label: impl Into<String>, command: Command) -> Self {
        self.menu = self.menu.item(label, command);
        self
    }

    pub fn separator(mut self) -> Self {
        self.menu = self.menu.separator();
        self
    }
}

impl Default for TraySpec {
    fn default() -> Self {
        Self::new()
    }
}
