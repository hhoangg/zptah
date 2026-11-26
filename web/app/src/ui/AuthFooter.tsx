import { cn } from "@/lib/utils";

/**
 * Footer of the sign-in family (login, register, recover): a small muted link to the privacy policy under the auth
 * card. A plain anchor, not a router Link: /privacy is a static page served by the Worker, not an SPA route, so it
 * needs a full page navigation (the service worker never answers it with the shell, see sw/routing.ts).
 */
export function AuthFooter({ className }: { className?: string }) {
  return (
    <footer data-testid="auth-footer" className={cn("flex justify-center pt-6 text-meta text-muted-text", className)}>
      <a href="/privacy" className="rounded-sm py-1 underline-offset-2 hover:text-content hover:underline">
        Privacy
      </a>
    </footer>
  );
}
