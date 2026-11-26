#!/usr/bin/env bash
set -euo pipefail

# 检查 Git 实际暂存树或 HEAD，而不是仅检查当前工作目录的文件名。
# 用法：scripts/check-source-only.sh --staged | --tracked

mode="${1:-}"
if [[ "$mode" != "--staged" && "$mode" != "--tracked" ]]; then
  printf '用法：%s --staged | --tracked\n' "$0" >&2
  exit 2
fi

for command in git tar file find; do
  if ! command -v "$command" >/dev/null 2>&1; then
    printf 'SOURCE_ONLY_ERROR 缺少命令：%s\n' "$command" >&2
    exit 2
  fi
done

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(git -C "$script_dir" rev-parse --show-toplevel)"
cd "$repo_root"

if [[ "$mode" == "--staged" ]]; then
  tree="$(git write-tree)"
else
  if ! git rev-parse --verify HEAD >/dev/null 2>&1; then
    printf 'SOURCE_ONLY_ERROR 当前还没有 HEAD；首次提交前请使用 --staged。\n' >&2
    exit 2
  fi
  tree="HEAD"
fi

snapshot="$(mktemp -d "${TMPDIR:-/tmp}/csm-source-only.XXXXXX")"
trap 'rm -rf "$snapshot"' EXIT HUP INT TERM

git archive "$tree" | tar -xf - -C "$snapshot"

errors=0
files_checked=0
max_file_bytes=$((5 * 1024 * 1024))

reject() {
  printf 'SOURCE_ONLY_ERROR path=%q reason=%s\n' "$1" "$2" >&2
  errors=$((errors + 1))
}

while IFS= read -r -d '' path; do
  relative="${path#"$snapshot"/}"
  reject "$relative" '不允许提交符号链接，避免链接到仓库外附件'
done < <(find "$snapshot" -type l -print0)

while IFS= read -r -d '' path; do
  relative="${path#"$snapshot"/}"
  lower="$(printf '%s' "$relative" | tr '[:upper:]' '[:lower:]')"
  base="${lower##*/}"
  files_checked=$((files_checked + 1))

  case "$lower" in
    *.dmg|*.exe|*.msi|*.msix|*.msixbundle|*.appimage|*.deb|*.rpm|*.pkg|*.apk|*.aab|*.ipa|*.iso|*.img|*.zip|*.7z|*.rar|*.tar|*.tar.*|*.tgz|*.gz|*.bz2|*.xz|*.zst|*.z)
      reject "$relative" '安装包、磁盘镜像或压缩附件不得进入源码仓库'
      continue
      ;;
    *.dll|*.dylib|*.so|*.so.*|*.a|*.lib|*.o|*.obj|*.pdb|*.node|*.wasm|*.bin|*.class|*.jar|*.app)
      reject "$relative" '编译载荷或二进制附件不得进入源码仓库'
      continue
      ;;
    *.jsonl|*.ndjson|*.sqlite|*.sqlite3|*.db|*.log)
      reject "$relative" '真实会话、数据库、缓存或日志不得进入源码仓库'
      continue
      ;;
    *.pem|*.key|*.p8|*.p12|*.pfx|*.cer|*.crt|*.der|*.mobileprovision|*.provisionprofile|*.keystore|*.jks|*.secret)
      reject "$relative" '凭据或签名材料不得进入源码仓库'
      continue
      ;;
    *.tmp|*.bak|*.backup|*.old|*.pdf)
      reject "$relative" '临时文件、备份或非源码附件不得进入源码仓库'
      continue
      ;;
  esac

  if [[ "$base" == ".env" || ( "$base" == .env.* && "$base" != ".env.example" ) ]]; then
    reject "$relative" '本地环境配置不得进入源码仓库'
    continue
  fi

  case "/$lower" in
    */.gitmodules|*/favorites.json|*/update-settings.json|*/session-metadata-v1.json|*/state_5.sqlite|*sums.txt|*checksums*.txt)
      reject "$relative" '子模块、本地运行数据或发布校验附件不在源码仓库允许范围内'
      continue
      ;;
  esac

  case "/$lower/" in
    */.claude/projects/*|*/.codex/*|*/.smoke-tmp/*|*/target/*|*/node_modules/*|*/dist/*|*/artifacts/*|*/delivery/*|*/deliverables/*|*.app/*|*.dsym/*|*.xcarchive/*)
      reject "$relative" '构建目录、交付目录或真实 Provider 数据不得进入源码仓库'
      continue
      ;;
  esac

  size="$(wc -c < "$path" | tr -d '[:space:]')"
  if (( size > max_file_bytes )); then
    reject "$relative" "单文件超过 5 MiB（实际 ${size} 字节）"
    continue
  fi

  if (( size > 0 )) && LC_ALL=C grep -q -m 1 '^version https://git-lfs.github.com/spec/' "$path"; then
    reject "$relative" '不允许用 Git LFS 指针绕过附件限制'
    continue
  fi

  mime="$(file -b --mime-type -- "$path")"
  case "$mime" in
    text/*|application/json|application/ld+json|application/xml|application/javascript|application/x-javascript|application/toml|application/yaml|application/x-yaml|application/x-sh|application/x-powershell|application/x-empty|inode/x-empty)
      ;;
    image/png|image/jpeg|image/svg+xml|image/vnd.microsoft.icon|image/x-icon)
      case "$lower" in
        *.png|*.jpg|*.jpeg|*.svg|*.ico)
          ;;
        *)
          reject "$relative" "图片 MIME ${mime} 与源码资源扩展名不一致"
          continue
          ;;
      esac
      ;;
    *)
      reject "$relative" "非源码 MIME 类型：${mime}"
      continue
      ;;
  esac
done < <(find "$snapshot" -type f -print0)

if (( errors > 0 )); then
  printf 'SOURCE_ONLY_FAILED mode=%s files=%d errors=%d\n' "$mode" "$files_checked" "$errors" >&2
  exit 1
fi

printf 'SOURCE_ONLY_OK mode=%s files=%d max_file_bytes=%d attachments=0\n' \
  "$mode" "$files_checked" "$max_file_bytes"
