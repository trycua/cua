// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { ExternalLinkIcon, Loader2Icon } from "lucide-react";
import { useState } from "react";

import type { CiAction, CiConnectView } from "@/bridge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogDescription, DialogPopup, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";

/**
 * "Connect GitHub": GitHub opens in the browser, the person picks the
 * organization and repositories there, and the page carries on by itself
 * when GitHub says it is done. "Use a token instead" is the way without the
 * Cua app on GitHub: an organization and a token (kept on this computer,
 * never sent to Cua). Every word is the core's.
 */
export function ConnectSheet({
  view,
  onAction,
  onOpen,
}: {
  view: CiConnectView;
  onAction(a: CiAction, extra?: { token?: string }): void;
  onOpen(url: string): void;
}) {
  // The token lives here only: it is sent with the press and then forgotten.
  const [token, setToken] = useState("");
  const close = () => onAction({ type: "connectClose" });
  const submit = () => {
    onAction({ type: "tokenSubmit" }, { token });
    setToken("");
  };
  return (
    <Dialog open onOpenChange={(o) => !o && !view.busy && close()}>
      <DialogPopup data-ci-connect={view.phase} className="top-[14vh] w-[min(480px,calc(100vw-2rem))]">
        <div className="px-6 pt-5 pb-5">
          <DialogTitle className="text-[15px] font-semibold">{view.title}</DialogTitle>
          <DialogDescription className="mt-1 text-[13px] text-muted-foreground">{view.body}</DialogDescription>
          {view.token ? (
            <form
              className="mt-4 space-y-3"
              onSubmit={(e) => {
                e.preventDefault();
                if (view.token!.canSubmit && !view.busy) submit();
              }}
            >
              <label className="block">
                <span className="mb-1 block text-xs text-muted-foreground">{view.token.orgLabel}</span>
                <Input
                  autoFocus
                  value={view.token.org}
                  placeholder={view.token.orgPlaceholder}
                  autoCapitalize="none"
                  autoCorrect="off"
                  spellCheck={false}
                  data-ci-token-org=""
                  onChange={(e) => onAction({ type: "tokenOrg", value: e.currentTarget.value })}
                />
              </label>
              <label className="block">
                <span className="mb-1 block text-xs text-muted-foreground">{view.token.tokenLabel}</span>
                <Input
                  type="password"
                  value={token}
                  autoComplete="off"
                  spellCheck={false}
                  placeholder="github_pat_"
                  data-ci-token-value=""
                  onChange={(e) => setToken(e.currentTarget.value)}
                />
                <span className="mt-1 block text-xs text-muted-foreground">{view.token.tokenHelp}</span>
              </label>
              <Button type="button" size="sm" variant="ghost" className="-ml-2" data-ci-create-token="" onClick={() => onOpen(view.token!.createUrl)}>
                <ExternalLinkIcon />
                {view.token.createLabel}
              </Button>
            </form>
          ) : null}
          {view.error ? (
            <p role="alert" className="mt-3 text-[13px] text-destructive" data-ci-connect-error="">
              {view.error}
            </p>
          ) : null}
          <div className="mt-5 flex items-center justify-between gap-3">
            <div>
              {view.secondary ? (
                <Button variant="ghost" data-ci-use-token="" disabled={view.busy} onClick={() => onAction({ type: "useToken" })}>
                  {view.secondary}
                </Button>
              ) : null}
            </div>
            <div className="flex items-center gap-2">
              <Button
                variant="outline"
                data-ci-connect-close=""
                disabled={view.busy && view.phase !== "waiting"}
                onClick={() => (view.phase === "token" ? onAction({ type: "tokenBack" }) : close())}
              >
                {view.close}
              </Button>
              {view.phase === "waiting" ? (
                <Button data-ci-connect-primary="" onClick={() => view.url && onOpen(view.url)}>
                  <Loader2Icon className="animate-spin motion-reduce:animate-none" aria-hidden />
                  {view.primary}
                </Button>
              ) : view.phase === "token" ? (
                <Button data-ci-connect-primary="" disabled={view.busy || !view.token?.canSubmit} onClick={submit}>
                  {view.busy ? <Loader2Icon className="animate-spin motion-reduce:animate-none" aria-hidden /> : null}
                  {view.primary}
                </Button>
              ) : (
                <Button data-ci-connect-primary="" disabled={view.busy} onClick={() => onAction({ type: "connectStart" })}>
                  {view.busy ? <Loader2Icon className="animate-spin motion-reduce:animate-none" aria-hidden /> : null}
                  {view.primary}
                </Button>
              )}
            </div>
          </div>
        </div>
      </DialogPopup>
    </Dialog>
  );
}
