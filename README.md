# mouse

鼠标手势工具：按住右键拖动手势，松开后向当前应用发送对应的快捷键组合，并保留右键单击（弹出上下文菜单）的能力。

## 适用系统

- **Linux**：X11 环境（通过 X 全局按键抓取 + WM_CLASS 识别应用），不支持 Wayland。
- **Windows**：Windows 10/11（通过低层鼠标钩子 + 进程名识别应用）。

## 手势与配置

配置文件为 `gestures.toml`，位于可执行文件同目录或当前工作目录。手势串由 1~3 段方向组成，用逗号分隔；方向取 8 个值（顺时针）：

```
R  右      DR  右下      D  下      DL  左下
L  左      UL  左上      U  上      UR  右上
```

规则按应用分组，名称用 `--identify` 模式查看：

```toml
[default]
"DR" = ["ctrl", "w"]      # 右下 → 关闭标签页
"L" = ["alt", "left"]     # 左滑 → 返回上一页
"U" = ["ctrl", "w"]       # 上滑 → 关闭标签页
"R,U" = ["ctrl", "t"]     # 右再上 → 新建标签页

[app."firefox_firefox"]
"R" = ["alt", "right"]    # 右滑 → 前进
"UL" = ["ctrl", "shift", "t"]  # 左上 → 打开最近关闭的标签
```

未匹配到应用时回退到 `[default]`。可用的修饰键与按键名见 `src/action.rs` 中的 `parse_key`。

## 使用

```
# 查看当前前台应用标识（应用分组名）
mouse --identify

# 启动手势监听
mouse
```

## 构建

轨迹渲染是可选功能（`trail` feature）。**默认关闭**：编译出的版本不带轨迹，等同于基础手势工具；开启后增加轨迹叠加层。

```
# 基础版（无轨迹）
cargo build --release
cargo build --release --target x86_64-pc-windows-gnu   # Windows

# 带轨迹版（开启 trail feature）
cargo build --release --features trail
cargo build --release --features trail --target x86_64-pc-windows-gnu   # Windows
```

带轨迹的版本可用 `--overlay-test` 验证轨迹是否正常工作：

```
mouse --overlay-test
```

## 已知限制

- Linux 需要 X11 会话；Wayland 下无法抓取全局鼠标。