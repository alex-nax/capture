# ASR Onboarding — handoff package

Everything needed to plan and build the ASR (speech-to-text) onboarding feature, and nothing outside
its scope.

## Contents

- **`ASR-ONBOARDING-PLAN.md`** — the implementation plan: states → frames, daemon API per action,
  components to reuse, build order, locked decisions. **Start here.**
- **`references/ASR-ONBOARDING-BRIEF.md`** — the original product brief (requirements).
- **`references/design-reference.md`** — scoped tokens + component specs this feature uses.
- **`references/*.png`** — the high-fidelity mocks, one per surface:
  - `1-dashboard-cta-hero.png` — state 1, the hero CTA
  - `2-cta-recovery.png` — minimised pill + partials + offline (states 1b, 6)
  - `3-dashboard-ready.png` — state 5, the resolved confirmation
  - `4-settings-voice-engine.png` — state 2, runtime picker
  - `5-settings-voice-model.png` — states 3 + 4, model picker + download

## Live, clickable mocks

The interactive source of truth lives in the project file **`Capture Screens.dc.html`** — the frames
labelled `ASR ·`. Open it to click through runtime switching and inspect exact styles.
