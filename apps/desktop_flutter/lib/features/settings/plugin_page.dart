import 'package:dsh_client/dsh_client.dart';
import 'package:flutter/material.dart';

import '../../src/controller.dart';
import 'resource_page.dart';

/// Full workspace plugin inventory, sharing the guarded settings actions.
/// Its frame matches the knowledge and schedule pages.
class PluginPage extends StatelessWidget {
  const PluginPage({
    super.key,
    required this.controller,
    this.onOpenPlugin,
    this.onClose,
    this.footer,
  });

  final DesktopController controller;
  final ValueChanged<Json>? onOpenPlugin;
  final VoidCallback? onClose;
  final Widget? footer;

  @override
  Widget build(BuildContext context) => LayoutBuilder(
    builder: (context, constraints) {
      final wide = constraints.maxWidth >= 880;
      return Padding(
        padding: EdgeInsets.fromLTRB(wide ? 28 : 16, 20, wide ? 28 : 16, 12),
        child: Align(
          alignment: Alignment.topLeft,
          child: ConstrainedBox(
            // Rows stay readable: the switch sits near its description.
            constraints: const BoxConstraints(maxWidth: 1040),
            child: SettingsResourcePage(
              controller: controller,
              page: 'plugins',
              workspace: true,
              onOpenPlugin: onOpenPlugin,
              onClose: onClose,
              footer: footer,
            ),
          ),
        ),
      );
    },
  );
}
