# Linux 桌面入口

解压后的目录可直接运行 `./dsh_desktop`，移动整个目录仍可运行。

在最终存放目录中执行以下命令，可将应用注册到当前用户的启动器；需要 Python 3，无需管理员权限。

```sh
python3 ./install-desktop-entry.py --install
```

入口写入 `$XDG_DATA_HOME/applications/io.deepseek.harness.dsh_desktop.desktop`，图标写入 `$XDG_DATA_HOME/icons/hicolor/scalable/apps/io.deepseek.harness.dsh_desktop.svg`。未设置 `XDG_DATA_HOME` 时采用 `~/.local/share`。应用文件、Host、会话和设置继续保存在原有位置。移动应用目录后，从新位置重新执行安装命令。

移除当前目录注册的桌面入口：

```sh
python3 ./install-desktop-entry.py --uninstall
```

卸载仅移除此目录对应的启动器入口及未修改的图标，保留应用文件、会话、设置和其他版本注册的入口。随后可按需要移除应用目录。`--data-home /absolute/path` 可指定独立的桌面数据目录；安装和卸载须使用同一目录。

面向系统软件包的 `.desktop` 文件与 hicolor SVG 位于 `share/`，由软件包管理器安装到对应共享数据目录，`dsh_desktop` 应位于启动器的 `PATH` 中。窗口标识、启动器文件名与图标名称均为 `io.deepseek.harness.dsh_desktop`。
