import 'package:dsh_client/dsh_client.dart';
import 'package:flutter/material.dart';

import '../../src/controller.dart';
import 'resource_page.dart';

/// Full workspace plugin inventory, sharing the guarded settings actions.
class PluginPage extends StatelessWidget {
  const PluginPage({
    super.key,
    required this.controller,
    this.onOpenPlugin,
    this.footer,
  });

  final DesktopController controller;
  final ValueChanged<Json>? onOpenPlugin;
  final Widget? footer;

  @override
  Widget build(BuildContext context) => LayoutBuilder(
    builder: (context, constraints) => Padding(
      padding: EdgeInsets.fromLTRB(
        constraints.maxWidth < 600 ? 20 : 40,
        constraints.maxHeight < 540 ? 20 : 32,
        constraints.maxWidth < 600 ? 20 : 40,
        12,
      ),
      child: Align(
        alignment: Alignment.topCenter,
        child: ConstrainedBox(
          constraints: const BoxConstraints(maxWidth: 1060),
          child: SettingsResourcePage(
            controller: controller,
            page: 'plugins',
            workspace: true,
            onOpenPlugin: onOpenPlugin,
            footer: footer,
          ),
        ),
      ),
    ),
  );
}
