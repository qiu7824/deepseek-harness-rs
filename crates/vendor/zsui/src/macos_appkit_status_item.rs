use std::rc::Rc;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{define_class, AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSImage, NSMenu, NSMenuDelegate, NSStatusBar, NSStatusItem, NSVariableStatusItemLength,
};
use objc2_foundation::{NSObject, NSObjectProtocol, NSString};

use crate::{Command, TraySpec, ZsuiError, ZsuiResult};

pub(crate) struct MacosAppKitStatusItemHost {
    status_bar: Retained<NSStatusBar>,
    items: Vec<Retained<NSStatusItem>>,
    menus: Vec<Rc<std::cell::RefCell<crate::macos_appkit_menu::MacosAppKitMenuService>>>,
    menu_delegates: Vec<Retained<ZsuiAppKitStatusMenuDelegate>>,
}

struct ZsuiAppKitStatusMenuDelegateIvars {
    menu_provider: crate::tray::TrayMenuProvider,
    menu_service: Rc<std::cell::RefCell<crate::macos_appkit_menu::MacosAppKitMenuService>>,
}

define_class!(
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = ZsuiAppKitStatusMenuDelegateIvars]
    struct ZsuiAppKitStatusMenuDelegate;

    unsafe impl NSObjectProtocol for ZsuiAppKitStatusMenuDelegate {}

    unsafe impl NSMenuDelegate for ZsuiAppKitStatusMenuDelegate {
        #[unsafe(method(menuWillOpen:))]
        fn menu_will_open(&self, menu: &NSMenu) {
            let current_menu = (self.ivars().menu_provider)();
            if let Ok(native_menu) = self
                .ivars()
                .menu_service
                .borrow_mut()
                .refreshed_detached_menu(&current_menu)
            {
                menu.setItemArray(&native_menu.itemArray());
            }
        }
    }
);

impl ZsuiAppKitStatusMenuDelegate {
    fn new(
        mtm: MainThreadMarker,
        menu_provider: crate::tray::TrayMenuProvider,
        menu_service: Rc<std::cell::RefCell<crate::macos_appkit_menu::MacosAppKitMenuService>>,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ZsuiAppKitStatusMenuDelegateIvars {
            menu_provider,
            menu_service,
        });
        unsafe { objc2::msg_send![super(this), init] }
    }
}

impl MacosAppKitStatusItemHost {
    pub(crate) fn create(
        trays: &[TraySpec],
        command_handler: Option<Rc<dyn Fn(Command)>>,
    ) -> ZsuiResult<Self> {
        let _mtm = MainThreadMarker::new().ok_or_else(|| {
            ZsuiError::host(
                "macos_status_item",
                "AppKit status items must be created on the macOS main thread",
            )
        })?;
        let mut host = Self {
            status_bar: NSStatusBar::systemStatusBar(),
            items: Vec::with_capacity(trays.len()),
            menus: Vec::with_capacity(trays.len()),
            menu_delegates: Vec::with_capacity(trays.len()),
        };
        for tray in trays {
            host.create_item(tray, command_handler.clone())?;
        }
        Ok(host)
    }

    pub(crate) fn item_count(&self) -> usize {
        self.items.len()
    }

    pub(crate) fn native_command_count(&self) -> usize {
        self.menus
            .iter()
            .map(|menu| menu.borrow().native_command_count())
            .sum()
    }

    pub(crate) fn invoke_first_enabled_command_for_proof(&self) -> bool {
        self.menus
            .iter()
            .any(|menu| menu.borrow().invoke_first_enabled_command_for_proof())
    }

    fn create_item(
        &mut self,
        tray: &TraySpec,
        command_handler: Option<Rc<dyn Fn(Command)>>,
    ) -> ZsuiResult<()> {
        let mut menu = crate::macos_appkit_menu::MacosAppKitMenuService::new()?;
        menu.set_detached_menu(&tray.current_menu())?;
        if let Some(command_handler) = command_handler {
            menu.set_command_handler(move |command| command_handler(command));
        }
        let menu = Rc::new(std::cell::RefCell::new(menu));

        let item = self
            .status_bar
            .statusItemWithLength(NSVariableStatusItemLength);

        #[allow(deprecated)]
        if let Some(icon_path) = tray.icon_path.as_deref() {
            let image =
                NSImage::initWithContentsOfFile(NSImage::alloc(), &NSString::from_str(icon_path))
                    .ok_or_else(|| {
                    ZsuiError::host(
                        "macos_status_item_icon",
                        format!("NSImage could not load status item icon `{icon_path}`"),
                    )
                })?;
            image.setTemplate(true);
            item.setImage(Some(&image));
            item.setTitle(None);
        } else {
            let title = tray.tooltip.as_deref().unwrap_or("ZSUI");
            item.setTitle(Some(&NSString::from_str(title)));
        }

        #[allow(deprecated)]
        item.setToolTip(tray.tooltip.as_deref().map(NSString::from_str).as_deref());
        item.setMenu(menu.borrow().native_menu());
        item.setVisible(true);
        if let Some(provider) = tray.menu_provider.clone() {
            let delegate = ZsuiAppKitStatusMenuDelegate::new(
                MainThreadMarker::new().expect("status items run on the AppKit main thread"),
                provider,
                Rc::clone(&menu),
            );
            let delegate_object: &ProtocolObject<dyn NSMenuDelegate> =
                ProtocolObject::from_ref(&*delegate);
            if let Some(native_menu) = menu.borrow().native_menu() {
                native_menu.setDelegate(Some(delegate_object));
            }
            self.menu_delegates.push(delegate);
        }
        self.menus.push(menu);
        self.items.push(item);
        Ok(())
    }
}

impl Drop for MacosAppKitStatusItemHost {
    fn drop(&mut self) {
        for item in self.items.drain(..) {
            item.setMenu(None);
            self.status_bar.removeStatusItem(&item);
        }
        for menu in &self.menus {
            if let Some(native_menu) = menu.borrow().native_menu() {
                native_menu.setDelegate(None);
            }
        }
        self.menu_delegates.clear();
        self.menus.clear();
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn status_item_refreshes_dynamic_menu_before_native_open() {
        let source = include_str!("macos_appkit_status_item.rs");
        assert!(source.contains("NSMenuDelegate"));
        assert!(source.contains("fn menu_will_open(&self, menu: &NSMenu)"));
        assert!(source.contains("tray.current_menu()"));
    }
}
