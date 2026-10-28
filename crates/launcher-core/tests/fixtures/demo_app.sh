#!/bin/sh
# app_catch 的 Unix 测试用小应用：模拟一个典型的"开发服务器"进程树。
#
#   demo_app.sh（组长，捕获 SIGTERM 后优雅退出）
#   ├── worker：组内子进程，每 200ms 打一行心跳
#   └── detached：用 perl setpgrp 脱离进程组的孙进程（模拟 detached / setsid 的子进程）
#
# 第一个参数是 pid 文件目录：子孙进程把自己的 pid 写进去，测试据此确认它们都被回收。
# 不传时写到 $TMPDIR，便于在界面里当普通应用手动启动。

PID_DIR="${1:-${TMPDIR:-/tmp}/app_catch_demo}"
mkdir -p "$PID_DIR"

echo "demo-app 启动，pid=$$"
echo "demo-app 这一行走 stderr" >&2

# 组内 worker
(
  trap 'exit 0' TERM
  i=0
  while :; do
    i=$((i + 1))
    echo "worker 心跳 $i"
    sleep 0.2
  done
) &
echo "$!" > "$PID_DIR/worker.pid"

# 脱组的孙进程：新进程组，killpg 打不到它，只能靠杀树前的父子关系快照
perl -e 'setpgrp(0, 0); open(my $f, ">", $ARGV[0]) or die; print $f $$; close $f; sleep 300' \
  "$PID_DIR/detached.pid" &

trap 'echo "demo-app 收到 SIGTERM，正在退出"; exit 0' TERM
echo "demo-app 就绪"
while :; do
  sleep 0.2
done
