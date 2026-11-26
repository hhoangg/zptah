import { cn } from "@/lib/utils";

/**
 * Brand block for the sign-in family (login, register, recover): the app icon and the lowercase name, centered
 * above the auth card. It replaces the top nav bar on those pages. Plain text, not a heading (each screen owns
 * its single h1) and not a link (the auth card's own links and the AuthFooter privacy link are the ways out).
 */
export function AuthBrand({ className }: { className?: string }) {
  return (
    <div
      data-testid="auth-brand"
      className={cn(
        "flex items-center justify-center gap-2.5 pt-[calc(env(safe-area-inset-top)+2rem)] sm:pt-[calc(env(safe-area-inset-top)+3rem)]",
        className,
      )}
    >
      <img src="/favicon.svg" alt="" width={36} height={36} className="size-9 rounded-[9px]" />
      <span className="text-[1.375rem] font-semibold tracking-tight text-content">zptah</span>
    </div>
  );
}
