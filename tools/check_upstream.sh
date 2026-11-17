#!/usr/bin/env bash
# Checks whether the CS2Toolkit files this project was ported from have changed
# since the commit recorded in .github/upstream-baseline, and opens an issue if so.
# 检查本项目移植所依据的 CS2Toolkit 文件，自 .github/upstream-baseline 记录的提交以来
# 是否有变化；如果有，就开一个 Issue。
#
# Needs the GitHub CLI (gh). Set DRY_RUN=1 to print the issue instead of creating it.
# 需要 GitHub CLI（gh）。设置 DRY_RUN=1 时只打印 Issue 内容而不创建。
set -euo pipefail

UPSTREAM="thok404/CS2Toolkit"
BRANCH="main"
LABEL="upstream-update"
# The files the switching logic was ported from. 切屏逻辑移植自这些文件。
WATCHED=(
  app/logic/visual_handler.py
  app/logic/application_windows.py
  app/logic/input_release.py
  app/logic/cs2_window.py
)

cd "$(dirname "$0")/.."
BASE="$(tr -d '[:space:]' < .github/upstream-baseline)"
HEAD="$(gh api "repos/$UPSTREAM/commits/$BRANCH" --jq .sha)"
echo "baseline: $BASE"
echo "upstream: $HEAD"

changed=()
for path in "${WATCHED[@]}"; do
  # Newest upstream commit that touched this file. 最近一次改动该文件的上游提交。
  latest="$(gh api "repos/$UPSTREAM/commits?path=$path&sha=$BRANCH&per_page=1" --jq '.[0].sha // empty')"
  if [ -z "$latest" ]; then
    changed+=("$path (removed or renamed 已删除或改名)")
    continue
  fi
  # "ahead" means that commit is newer than the baseline. "ahead" 表示该提交比基线新。
  status="$(gh api "repos/$UPSTREAM/compare/$BASE...$latest" --jq .status)"
  if [ "$status" = "ahead" ] || [ "$status" = "diverged" ]; then
    changed+=("$path")
  fi
done

if [ "${#changed[@]}" -eq 0 ]; then
  echo "No changes in the watched files. 关注的文件没有变化。"
  exit 0
fi

short="${HEAD:0:7}"
title="上游切屏逻辑有更新 / Upstream switching logic changed ($short)"
body="$(
  echo "CS2Toolkit 中与切屏相关的文件在上次移植之后有了改动，可能需要同步到 CSTool。"
  echo
  echo "Files related to screen switching changed in CS2Toolkit since the last port; CSTool may need the same change."
  echo
  echo "## 有变化的文件 / Changed files"
  echo
  for path in "${changed[@]}"; do
    file="${path%% *}"
    echo "- \`$path\` ([历史 history](https://github.com/$UPSTREAM/commits/$BRANCH/$file))"
  done
  echo
  echo "## 对比 / Compare"
  echo
  echo "https://github.com/$UPSTREAM/compare/$BASE...$HEAD"
  echo
  echo "## 处理完之后 / When done"
  echo
  echo "把 \`.github/upstream-baseline\` 的内容改为 \`$HEAD\`，然后关闭这个 Issue。即使改动无需移植，也请这样做，否则每天都会再次提醒。"
  echo
  echo "Set \`.github/upstream-baseline\` to \`$HEAD\` and close this issue. Do this even if nothing needed porting, otherwise the reminder repeats."
)"

if [ "${DRY_RUN:-}" = "1" ]; then
  echo "--- would open issue: $title"
  echo "$body"
  exit 0
fi

# Only one reminder at a time: skip while an earlier one is still open.
# 同一时间只保留一个提醒：之前的提醒还没关闭时跳过。
open_count="$(gh issue list --state open --label "$LABEL" --json number --jq 'length')"
if [ "$open_count" != "0" ]; then
  echo "A reminder is already open. 已有一个未关闭的提醒。"
  exit 0
fi
gh label create "$LABEL" --color FBCA04 --description "CS2Toolkit changed files this project was ported from" --force
gh issue create --title "$title" --body "$body" --label "$LABEL"
