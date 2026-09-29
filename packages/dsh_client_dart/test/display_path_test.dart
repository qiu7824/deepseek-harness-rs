import 'package:dsh_client/dsh_client.dart';
import 'package:test/test.dart';

void main() {
  test('display strips only extended drive and UNC namespaces', () {
    expect(displayPath(r'\\?\C:\项目\a.txt'), r'C:\项目\a.txt');
    expect(displayPath(r'\\?\unc\server\share\a.txt'), r'\\server\share\a.txt');
    expect(displayPath(r'\\server\share\a.txt'), r'\\server\share\a.txt');
    expect(displayPath(r'\\?\Volume{abc}\file'), r'\\?\Volume{abc}\file');
    expect(displayPath(r'\\.\pipe\name'), r'\\.\pipe\name');
    expect(displayPath(r'C:\项目\a.txt'), r'C:\项目\a.txt');
    expect(
      displayPathText(r'目录：\\?\C:\项目；共享：\\?\UNC\host\share'),
      r'目录：C:\项目；共享：\\host\share',
    );
    expect(toolPathLabel(r'\\?\C:\项目\a.txt', null), r'C:\项目\a.txt');
    expect(toolPathLabel(r'\\?\D:\a.txt', r'C:\项目'), r'D:\a.txt');
    expect(
      DshException('path', r'无法读取 \\?\C:\项目\a.txt').toString(),
      r'无法读取 C:\项目\a.txt (path)',
    );
  });
  test('workspace folders compare across prefixes, separators and casing', () {
    final key = workspacePathKey(r'E:\资料\交通\龙江路');
    expect(workspacePathKey(r'\\?\E:\资料\交通\龙江路'), key);
    expect(workspacePathKey(r'e:/资料/交通/龙江路/'), key);
    expect(workspacePathKey(r'E:\资料\交通'), isNot(key));
    expect(workspacePathKey(r'C:\'), r'c:\');
    expect(workspacePathKey(r'\\?\UNC\Host\Share\'), r'\\host\share');
    expect(workspacePathKey('/home/User/app/'), '/home/User/app');
    expect(workspacePathKey('/home/user/app'), isNot('/home/User/app'));
    expect(workspacePathKey('/'), '/');
    expect(workspacePathKey(null), '');
  });
}
