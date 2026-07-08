# Handoff Prompt: Codex Fork Rebase (runtime/day-to-day operational set)

Context
- Branch checked: `main`.
- Repo root: `/home/ericjuta/.openclaw/workspace/repos/codex`.
- Current objective from latest chat: pick only selected upstream runtime commits (indices 7,8,9,10,12,13,14,15,16,17,18,19,20,21,22) from `origin/main` for operational Codex sessions.
- `origin` is `https://github.com/openai/codex`.
- `git rev-list --left-right --count --no-merges HEAD...origin/main` reported local behind count from earlier discussion: 83 upstream vs 70 local-only.

Selected commits (in chronological order)
1. `2e20d2ef44` Revert "Conditional codex_home dotenv" (#31276)
2. `7b4e70d567` Revert "[core] Support interleaved response items" (#31261)
3. `8917244f7d` [core] Support interleaved response items (#30876)
4. `7094fa467e` [codex] Read retry model from buffering events (#31262)
5. `7affe3e3e4` refactor(protocol): isolate legacy item fanout (#30956)
6. `b9b934e99b` refactor(protocol): map canonical tool items to legacy events (#31296)
7. `cca16a1087` feat(core): emit canonical command execution items (#31297)
8. `f659eb12bc` feat(core): emit canonical dynamic tool call items (#31298)
9. `1bd9d841ca` feat(core): emit canonical sub-agent activity items (#31299)
10. `058d97c5dc` feat(core): emit canonical collab tool call items (#31300)
11. `6b4882528e` feat(core): emit canonical collab wait items (#31301)
12. `f1affbac5e` core: support extension-owned turn items (#31283)
13. `ff06ab7172` [codex] Enable auth elicitation by default (#28772)
14. `1fd0858e86` [login] support hosted success redirects (#28745)
15. `a3f8b0b332` refactor: make ExternalAuth return CodexAuth (#31355)

Why this set
- Focus: auth and auth-mediated runtime flow, execution/transcript event canonicalization, buffered response behavior, protocol event fanout shape, and extension-owned turn semantics.

Suggested next step
- Cherry-pick these from oldest to newest (or rebase onto `origin/main`, then keep these as baseline):
  `git fetch origin`
  `git checkout main`
  `git rebase origin/main`
- If you only want these 15 commits explicitly:
  `git cherry-pick 2e20d2ef44 7b4e70d567 8917244f7d 7094fa467e 7affe3e3e4 b9b934e99b cca16a1087 f659eb12bc 1bd9d841ca 058d97c5dc 6b4882528e f1affbac5e ff06ab7172 1fd0858e86 a3f8b0b332`
