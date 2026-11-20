# Repository Instructions

## Rust Rules

- In Rust code, do not introduce `unwrap()` or `expect()`.
- Allowed exceptions:
- Tests may use `unwrap()` or `expect()` when it keeps the test focused and readable.
- Lock acquisition may use `unwrap()` only when the locking API makes that the practical option and the failure mode is poison handling rather than normal control flow.
- Outside those exceptions, propagate errors, handle them explicitly, or use safer fallbacks instead of `unwrap()` and `expect()`.

## Editing Hygiene

- Do not introduce formatting-only changes.
- Do not run repository-wide formatters or reflow unrelated code unless the
  user explicitly asks for formatting.
- Keep diffs limited to semantic changes required for the task.

## Console UI Rules（控制台界面规则）

- 禁止冗余说明：按钮/功能通过图标、布局、交互传达，避免"开启后自动保存"等小字解释。
- 布局优先：用层级、间距、图标引导操作，不依赖文字说明。
- 简洁文案：仅保留核心提示（如"删除不可恢复"），删除重复描述。
- 交互自明：hover 提示、状态反馈替代静态说明。
- 代码规范：语义化 HTML，组件单一职责。控制台是零构建的原生 HTML/CSS/JS（内嵌于 `src/api_pages.rs`），不引入 Tailwind 等构建依赖。

