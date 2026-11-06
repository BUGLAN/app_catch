#!/usr/bin/env bash
# 生成 app_catch 的 macOS 自签名代码签名证书（一次性操作，生成后长期复用）。
#
# 为什么需要：macOS 的隐私授权（TCC）按「bundle id + 签名要求」记账。
# ad-hoc 签名的签名要求是 cdhash，每次发版都会变，用户的麦克风等授权随之失效；
# 固定证书签名后，签名要求变成 `identifier "com.appcatch.launcher" and certificate leaf = H"<证书指纹>"`，
# 只要证书不换，升级后授权一直有效。详见 docs/MACOS_PERMISSIONS.md。
#
# 用法：
#   scripts/gen-macos-signing-cert.sh            # 输出到 ~/.app_catch-signing
#   OUT_DIR=/path/to/dir scripts/gen-macos-signing-cert.sh
#
# 产出（目录权限 700，**不要提交进仓库，务必另行备份**，丢了只能换证书，所有用户的授权会失效一次）：
#   key.pem / cert.pem   私钥与证书原件
#   cert.p12             给 CI 导入用的 PKCS#12 包
#   password.txt         cert.p12 的密码
#
# 之后把下面两项配置为 GitHub 仓库 secret（脚本结尾会打印具体命令）：
#   MACOS_SIGNING_CERT_P12       cert.p12 的 base64
#   MACOS_SIGNING_CERT_PASSWORD  password.txt 的内容
#
# 注意 secret 名字刻意**不用** APPLE_CERTIFICATE：Tauri 读到它会走自己的导入流程，
# 那条流程只认 Apple 签发的证书（CN 前缀 + Team ID），自签名证书会直接报错。

set -euo pipefail

CN="app_catch Self-Signed Code Signing"
DAYS=7300 # 20 年：证书过期后要换证书，换证书 = 所有授权失效一次
OUT_DIR="${OUT_DIR:-$HOME/.app_catch-signing}"
# 系统自带的 LibreSSL 导出的 p12 能被 `security import` 直接识别；
# Homebrew 的 OpenSSL 3 默认算法 security 不认，必须加 -legacy，所以固定用系统版本。
OPENSSL=/usr/bin/openssl

if [ -e "$OUT_DIR/cert.p12" ]; then
  echo "已存在 $OUT_DIR/cert.p12，拒绝覆盖（换证书会让所有用户的授权失效一次）。" >&2
  echo "确实要重新生成，请先手动移走该目录。" >&2
  exit 1
fi

mkdir -p "$OUT_DIR"
chmod 700 "$OUT_DIR"
umask 077

cat > "$OUT_DIR/cert.cnf" <<EOF
[req]
distinguished_name = dn
x509_extensions = ext
prompt = no
[dn]
CN = $CN
[ext]
basicConstraints = critical,CA:false
keyUsage = critical,digitalSignature
extendedKeyUsage = critical,codeSigning
subjectKeyIdentifier = hash
EOF

"$OPENSSL" req -x509 -newkey rsa:3072 -nodes -sha256 -days "$DAYS" \
  -config "$OUT_DIR/cert.cnf" \
  -keyout "$OUT_DIR/key.pem" -out "$OUT_DIR/cert.pem" 2>/dev/null

PASSWORD="$("$OPENSSL" rand -hex 24)"
printf '%s' "$PASSWORD" > "$OUT_DIR/password.txt"

"$OPENSSL" pkcs12 -export \
  -inkey "$OUT_DIR/key.pem" -in "$OUT_DIR/cert.pem" \
  -name "$CN" -out "$OUT_DIR/cert.p12" -passout "pass:$PASSWORD"

FINGERPRINT="$("$OPENSSL" x509 -in "$OUT_DIR/cert.pem" -noout -fingerprint -sha1 | cut -d= -f2 | tr -d :)"

cat <<EOF
已生成自签名证书：
  名称      $CN
  SHA-1     $FINGERPRINT
  有效期    $DAYS 天
  目录      $OUT_DIR

下一步：配置 GitHub 仓库 secret（需要已登录 gh）：
  base64 -i "$OUT_DIR/cert.p12" | gh secret set MACOS_SIGNING_CERT_P12
  gh secret set MACOS_SIGNING_CERT_PASSWORD < "$OUT_DIR/password.txt"

然后务必把 $OUT_DIR 备份到安全的地方（密码管理器 / 加密存储）。
EOF
