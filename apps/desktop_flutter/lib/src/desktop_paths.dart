import 'package:path/path.dart' as p;

/// Desktop-owned data and bundled Host locations, independent of process cwd.
abstract final class DesktopPaths {
  static p.Context context(String os) =>
      p.Context(style: os == 'windows' ? p.Style.windows : p.Style.posix);

  static String preferences(String os, Map<String, String> environment) {
    final override = environment['DSH_DESKTOP_PREFERENCES'];
    if (override != null && override.trim().isNotEmpty) return override;
    final paths = context(os);
    final home = environment[os == 'windows' ? 'USERPROFILE' : 'HOME'];
    String root;
    if (os == 'windows') {
      root =
          environment['LOCALAPPDATA'] ??
          (home == null ? '' : paths.join(home, 'AppData', 'Local'));
    } else if (os == 'macos') {
      root = home == null
          ? ''
          : paths.join(home, 'Library', 'Application Support');
    } else {
      final configured = environment['XDG_CONFIG_HOME'];
      root = configured != null && paths.isAbsolute(configured)
          ? configured
          : home == null
          ? ''
          : paths.join(home, '.config');
    }
    if (root.isEmpty || !paths.isAbsolute(root)) {
      throw const FormatException('无法确定用户设置目录，请配置 DSH_DESKTOP_PREFERENCES。');
    }
    return paths.join(root, 'DeepSeek Harness Desktop', 'preferences.json');
  }

  static String hostName(String os) =>
      os == 'windows' ? 'deepseek-harness-rs.exe' : 'deepseek-harness-rs';
  static String nodeName(String os) => os == 'windows' ? 'node.exe' : 'node';

  static List<String> bundledHostRoots(String executableDir, String os) {
    final paths = context(os);
    return [
      if (os == 'macos' &&
          paths.basename(executableDir) == 'MacOS' &&
          paths.basename(paths.dirname(executableDir)) == 'Contents')
        paths.normalize(paths.join(executableDir, '..', 'Resources', 'host')),
      paths.join(executableDir, 'host'),
    ];
  }
}
