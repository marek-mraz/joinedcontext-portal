import { render, screen } from "@testing-library/react";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { LifecycleBadge } from "../src/components/status/LifecycleBadge";

function renderBadges(children: React.ReactNode) {
  return render(<I18nextProvider i18n={i18n}>{children}</I18nextProvider>);
}

describe("lifecycle badge", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("labels every phase the reconciler reports", () => {
    renderBadges(
      <>
        <LifecycleBadge kind="phase" value="Draft" />
        <LifecycleBadge kind="phase" value="PendingApproval" />
        <LifecycleBadge kind="phase" value="Deploying" />
        <LifecycleBadge kind="phase" value="Live" />
        <LifecycleBadge kind="phase" value="Error" />
        <LifecycleBadge kind="phase" value="Drifted" />
      </>,
    );

    for (const label of [
      en.phase.draft,
      en.phase.pendingApproval,
      en.phase.deploying,
      en.phase.live,
      en.phase.error,
      en.phase.drifted,
    ]) {
      expect(screen.getByText(label)).toBeInTheDocument();
    }
  });

  it("gives each variant its own colour scheme", () => {
    renderBadges(
      <>
        <LifecycleBadge kind="phase" value="Deploying" />
        <LifecycleBadge kind="phase" value="Live" />
        <LifecycleBadge kind="phase" value="Error" />
        <LifecycleBadge kind="phase" value="Drifted" />
      </>,
    );

    expect(screen.getByText(en.phase.deploying).className).toContain("info");
    expect(screen.getByText(en.phase.live).className).toContain("success");
    expect(screen.getByText(en.phase.error).className).toContain("danger");
    expect(screen.getByText(en.phase.drifted).className).toContain("purple");
  });

  it("explains each variant in a tooltip, so colour is never the only carrier", () => {
    renderBadges(
      <>
        <LifecycleBadge kind="phase" value="Deploying" />
        <LifecycleBadge kind="lane" value="red" />
      </>,
    );

    expect(screen.getByText(en.phase.deploying)).toHaveAttribute("title", en.phase.deployingHelp);
    expect(screen.getByText(en.lane.red)).toHaveAttribute("title", en.lane.redHelp);
  });

  it("colours the three risk lanes green, amber and red", () => {
    renderBadges(
      <>
        <LifecycleBadge kind="lane" value="green" />
        <LifecycleBadge kind="lane" value="yellow" />
        <LifecycleBadge kind="lane" value="red" />
      </>,
    );

    expect(screen.getByText(en.lane.green).className).toContain("success");
    expect(screen.getByText(en.lane.yellow).className).toContain("warning");
    expect(screen.getByText(en.lane.red).className).toContain("danger");
  });

  it("translates the label with the locale", async () => {
    await i18n.changeLanguage("sk");
    renderBadges(<LifecycleBadge kind="phase" value="Deploying" />);
    expect(screen.getByText("Nasadzuje sa")).toBeInTheDocument();
  });

  it("shows an unknown status verbatim instead of an empty chip", () => {
    renderBadges(<LifecycleBadge kind="phase" value="Hibernating" />);
    const chip = screen.getByText("Hibernating");
    expect(chip).not.toHaveAttribute("title");
    expect(chip.className).toContain("bg-surface-subtle");
  });
});
