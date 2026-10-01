import 'package:flutter/widgets.dart';

abstract final class DshBreakpoints {
  static const minimumWindow = Size(720, 520);
  static const sidebar = 900.0;
  static const workbench = 1100.0;
  static const readingMinimum = 760.0;
  static const readingMaximum = 920.0;

  static bool collapseSidebar(double width) => width < sidebar;
  static bool overlayWorkbench(double width) => width < workbench;
  static double readingWidth(double available) =>
      available.clamp(0, readingMaximum).toDouble();
}
