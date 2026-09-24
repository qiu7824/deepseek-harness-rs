import 'package:dsh_desktop/src/desktop_paths.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test(
    'settings persist in the platform user directory, never temporary data',
    () {
      expect(
        DesktopPaths.preferences('windows', {
          'LOCALAPPDATA': r'C:\Users\用户\AppData\Local',
        }),
        r'C:\Users\用户\AppData\Local\DeepSeek Harness Desktop\preferences.json',
      );
      expect(
        DesktopPaths.preferences('macos', {'HOME': '/Users/name'}),
        '/Users/name/Library/Application Support/DeepSeek Harness Desktop/preferences.json',
      );
      expect(
        DesktopPaths.preferences('linux', {'HOME': '/home/name'}),
        '/home/name/.config/DeepSeek Harness Desktop/preferences.json',
      );
      expect(
        DesktopPaths.preferences('linux', {
          'HOME': '/home/name',
          'XDG_CONFIG_HOME': '/data/config',
        }),
        '/data/config/DeepSeek Harness Desktop/preferences.json',
      );
      expect(
        DesktopPaths.preferences('linux', {
          'HOME': '/home/name',
          'XDG_CONFIG_HOME': 'relative',
        }),
        '/home/name/.config/DeepSeek Harness Desktop/preferences.json',
      );
      expect(
        () => DesktopPaths.preferences('linux', {}),
        throwsFormatException,
      );
    },
  );

  test('bundled Host follows desktop and macOS application layouts', () {
    expect(DesktopPaths.bundledHostRoots(r'D:\Programs\应用', 'windows'), [
      r'D:\Programs\应用\host',
    ]);
    expect(DesktopPaths.bundledHostRoots('/opt/dsh', 'linux'), [
      '/opt/dsh/host',
    ]);
    expect(
      DesktopPaths.bundledHostRoots(
        '/Applications/DeepSeek Harness.app/Contents/MacOS',
        'macos',
      ).first,
      '/Applications/DeepSeek Harness.app/Contents/Resources/host',
    );
    expect(DesktopPaths.hostName('macos'), 'deepseek-harness-rs');
    expect(DesktopPaths.nodeName('linux'), 'node');
    expect(DesktopPaths.hostName('windows'), 'deepseek-harness-rs.exe');
  });
}
