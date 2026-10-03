import 'package:flutter/material.dart';

abstract final class DshMotion {
  static const quick = Duration(milliseconds: 120);
  static const panel = Duration(milliseconds: 180);
  static const dialog = Duration(milliseconds: 240);
  static const curve = Curves.easeOutCubic;

  static bool disabled(BuildContext context) =>
      MediaQuery.disableAnimationsOf(context) ||
      MediaQuery.accessibleNavigationOf(context);

  static Duration duration(BuildContext context, Duration requested) =>
      disabled(context) ? Duration.zero : requested;
}
