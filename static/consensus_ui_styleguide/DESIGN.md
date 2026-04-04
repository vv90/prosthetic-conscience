# Design System Specification: Editorial Workbench

## 1. Overview & Creative North Star
**Creative North Star: "The Architectural Curator"**

This design system moves beyond the generic "SaaS dashboard" aesthetic to create a high-fidelity, editorial experience. It is designed to feel like a high-end physical workspace—clean, intentional, and authoritative. We achieve this by breaking the rigid, boxed-in nature of traditional UI. 

Instead of relying on borders to contain information, we use **intentional asymmetry, overlapping depth, and dramatic typographic scales** to guide the eye. The interface should feel "curated" rather than "assembled," utilizing breathing room (white space) as a structural element to elevate the sophisticated teal and amber palette.

---

## 2. Color & Tonal Depth

The palette is anchored in a deep, authoritative teal, balanced by a range of cool slates that create a focused "workbench" environment.

### The "No-Line" Rule
**Explicit Instruction:** Designers are prohibited from using 1px solid borders for sectioning or layout containment. Boundaries must be defined solely through:
1.  **Background Tonal Shifts:** Placing a `surface-container-low` component against a `surface` background.
2.  **Negative Space:** Utilizing the spacing scale to create "implied containers."

### Surface Hierarchy & Nesting
Treat the UI as a series of physical layers, like stacked sheets of fine paper or frosted glass. Use the hierarchy below to define importance:
*   **Lowest Layer:** `surface` (#f4faff) – The global canvas.
*   **Secondary Layer:** `surface-container-low` (#e9f6fd) – Sidebars or utility zones.
*   **Primary Focus:** `surface-container-lowest` (#ffffff) – High-priority content cards.
*   **Elevated Elements:** `surface-container-high` (#ddeaf2) – Active selection states or floating modals.

### The "Glass & Gradient" Rule
To avoid a flat "template" look, floating elements (modals, dropdowns) must use **Glassmorphism**. Apply a semi-transparent `surface` color with a `backdrop-blur` of 12px–20px. 
*   **Signature Textures:** For high-impact CTAs, use a subtle linear gradient (Top-Left to Bottom-Right) transitioning from `primary` (#00342b) to `primary-container` (#004d40). This provides a "jewel-toned" depth that feels premium.

---

## 3. Typography: The Editorial Voice

We pair the geometric precision of **Manrope** with the high-utility legibility of **Inter**.

*   **Manrope (Headlines/Display):** Used for brand expression and structural landmarks. It should feel spacious; use `letter-spacing: -0.02em` for `display-lg` through `headline-sm` to create a tight, professional lockup.
*   **Inter (Body/UI):** Used for the "work" areas. Inter excels in dense data environments. Maintain a strict `1.5` line-height for body text to ensure the "Editorial" feel isn't lost in data density.

| Level | Font | Size | Case/Weight | Use Case |
| :--- | :--- | :--- | :--- | :--- |
| **Display-LG** | Manrope | 3.5rem | Bold | Hero sections, high-impact landing areas |
| **Headline-MD**| Manrope | 1.75rem| SemiBold | Primary page headers |
| **Title-SM** | Inter | 1rem | Medium | Card titles, subsection headers |
| **Body-MD** | Inter | 0.875rem| Regular | Standard UI text, descriptions |
| **Label-MD** | Inter | 0.75rem | Bold (All Caps)| Metadata, "Disputed" Amber tags |

---

## 4. Elevation & Depth

We convey hierarchy through **Tonal Layering** rather than structural lines.

### Ambient Shadows
Shadows are used sparingly for "floating" objects (Modals, Popovers). 
*   **Formula:** `0px 12px 32px -4px`. 
*   **Color:** Do not use pure black. Use a 6% opacity version of `on-surface` (#111d23) to mimic natural ambient light.

### The "Ghost Border" Fallback
If a border is legally or functionally required for accessibility, use a **Ghost Border**. 
*   **Token:** `outline-variant` (#bfc9c4) at **15% opacity**. 
*   **Constraint:** Never use 100% opaque borders for interior layout separation.

### Corner Radii
We use a "Soft-Professional" scale to appear more approachable than legacy systems.
*   **Standard (md):** `0.75rem` (12px) – Default for cards and inputs.
*   **Large (lg):** `1rem` (16px) – For major layout containers.
*   **Full:** `9999px` – For Pill buttons and Chips.

---

## 5. Components

### Buttons
*   **Primary:** Fill `primary` (#00342b), Text `on-primary` (#ffffff). Use a subtle gradient (see section 2).
*   **Secondary:** Fill `secondary-container` (#cfe6f2), Text `on-secondary-container` (#526772). No border.
*   **Tertiary/Ghost:** No fill. Text `primary`. High-contrast interaction on hover (slight `surface-container` fill).

### Cards & Lists
*   **Strict Rule:** No divider lines between list items. Use `2.5rem` (10) or `3rem` (12) vertical spacing from the scale or a subtle background toggle (`surface-container-lowest` vs `surface-container-low`) to differentiate rows.
*   **Nesting:** Place a white card (`surface-container-lowest`) onto a slate background (`surface-container-low`) for a natural, shadowless lift.

### Input Fields
*   **Style:** `surface-container-lowest` background with a `0.5rem` (DEFAULT) radius. 
*   **Focus State:** Instead of a heavy border, use a `2px` outer glow in `primary-fixed` (#afefdd) and transition the background color slightly.

### High-Attention Status (Amber)
*   **Disputed/Action Required:** Use `tertiary_fixed_dim` (#fabd00) for backgrounds of status chips, with `on_tertiary_fixed` (#261a00) for text. This high-contrast amber provides the necessary "friction" in a sea of teal and slate.

---

## 6. Do’s and Don’ts

### Do
*   **Do** use asymmetrical layouts. For example, a wide content column paired with a narrow, offset metadata column.
*   **Do** utilize the `surface-tint` (#29695b) at low opacities for subtle background washes in specific "Focus Modes."
*   **Do** rely on Manrope’s large scale to create clear information architecture.

### Don’t
*   **Don't** use 1px dividers to separate menu items. Use vertical spacing.
*   **Don't** use pure gray shadows. Always tint shadows with the background hue for a "High-Fidelity" look.
*   **Don't** use Amber for decorative purposes. It is a functional color reserved for "Disputed" statuses and high-priority alerts.