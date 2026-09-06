import type { Metadata } from "next";
import { Big_Shoulders_Display, IBM_Plex_Sans, IBM_Plex_Mono } from "next/font/google";
import "./site.css";

// Google Fonts split "Big Shoulders" into Display/Text cuts a while back —
// there's no longer a single unified family name to import. Display is the
// right pick here: it's the cut tuned for large headline sizes, which is
// the only place this face is used (the h1 and the sheet-number labels).
const bigShoulders = Big_Shoulders_Display({
  subsets: ["latin"],
  weight: ["400", "600", "700", "800"],
  variable: "--font-display",
  display: "swap",
});

const plexSans = IBM_Plex_Sans({
  subsets: ["latin"],
  weight: ["400", "500", "600"],
  style: ["normal", "italic"],
  variable: "--font-sans",
  display: "swap",
});

const plexMono = IBM_Plex_Mono({
  subsets: ["latin"],
  weight: ["400", "500", "600"],
  variable: "--font-mono",
  display: "swap",
});

export const metadata: Metadata = {
  title: "Archietect — deterministic architectural memory for AI coding tools",
  description:
    "Archietect is a deterministic, evidence-backed memory of what a codebase is — shared across every project on the machine, queried identically by CLI, REST, and MCP.",
  // No manual `icons` entry — app/favicon.ico (Next's own file convention)
  // is what actually answers the browser's automatic GET /favicon.ico
  // request. The data-URI this used to point at only ever produced a
  // <link> tag; it never satisfied that separate, automatic request,
  // which is exactly what was 500ing in production.
  openGraph: {
    title: "Archietect",
    description:
      "A deterministic, evidence-backed memory of what a codebase is — shared across every project on the machine, not rebuilt from scratch per session.",
    type: "website",
  },
  twitter: { card: "summary" },
};

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en" className={`${bigShoulders.variable} ${plexSans.variable} ${plexMono.variable}`}>
      <body>{children}</body>
    </html>
  );
}
