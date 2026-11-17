# Complete Website Reverse Engineering & 1:1 Clone

## Role & Objective

Act as a team of senior full-stack engineers, UI/UX designers, software architects, QA engineers, and website reverse-engineering specialists.

Your mission is to **analyze, reconstruct, implement, and thoroughly test a complete 1:1 replica of the reference website**, including its visual appearance, content, structure, responsiveness, interactions, animations, and functional behavior.

**Reference website:** `{WEBSITE_URL}`

The final result must look and behave as closely as possible to the original website across desktop, tablet, and mobile devices.

This is NOT a creative redesign, simplified recreation, or proof of concept. The reference website is the specification.

Do not substitute its design with your own preferences.

The implementation must also have clean, maintainable, scalable, and secure code, even if the original website's implementation is outdated or poorly structured.

Only reproduce assets, content, and functionality that you own or are authorized to replicate.

---

# PHASE 1 - Deep Website Discovery & Analysis

## 1.1 Complete Website Crawling

Start by performing an extensive crawl and exploration of the reference website.

Discover and inspect:

- Homepage and every publicly accessible page.
- Navigation menus and dropdowns.
- Header and footer links.
- Every internal route and subpage.
- Landing pages.
- Products and product details.
- Services and service details.
- Blog posts, articles, news, and categories.
- Search results and filtering.
- Contact and information pages.
- Authentication and user account flows where authorized.
- User dashboards and protected areas where access is provided.
- Forms, modals, dialogs, and interactive components.
- Legal and policy pages.
- Multilingual content and language switching.
- Dynamic pages and parameterized routes.
- Sitemap.xml and robots.txt.
- Query parameters that affect displayed content.
- Error pages, empty states, and redirects.

Use browser automation and source inspection where available.

Inspect every unique page, not just the first few examples.

If pages use identical templates, analyze the shared template and document the unique content and behavior of each page.

## 1.2 Detailed Page Inventory

For each page, record:

- URL and route structure.
- Page title and purpose.
- Content and headings.
- Layout and section order.
- Header and footer variations.
- Visual components.
- Images, icons, and media.
- Buttons and interactive elements.
- Forms and validation rules.
- Navigation destinations.
- Responsive behavior.
- Loading, hover, focus, and error states.
- Animations and transitions.
- SEO metadata.
- Functional dependencies.

Do not consider a page analyzed until its appearance and significant behavior have been inspected.

## 1.3 Website Documentation

Create:

`docs/website-clone-analysis.md`

Include:

1. Website overview.
2. Business and website purpose.
3. Complete URL inventory.
4. Page-by-page content analysis.
5. Design system analysis.
6. Component inventory.
7. Functional requirements.
8. Responsive behavior.
9. Navigation architecture.
10. Data models and integrations.
11. Technical architecture.
12. Mermaid sitemap diagram.
13. Mermaid user-flow diagrams.
14. Identified unknowns and limitations.

This document will serve as the reference throughout implementation.

---

# PHASE 2 - Pixel-Accurate Design Reconstruction

## 2.1 Exact Visual Replication

Reconstruct the original visual design as closely as technically possible.

Match:

- Page dimensions and proportions.
- Content container widths.
- Section spacing.
- Margins and padding.
- Grid and flexbox arrangements.
- Alignment and positioning.
- Background colors and gradients.
- Typography and font weights.
- Font sizes and line heights.
- Letter spacing.
- Buttons and interactive elements.
- Borders and border radii.
- Shadows and visual effects.
- Image sizes and cropping.
- Icons and illustrations.
- Header and footer layouts.
- Cards and content sections.
- Sticky and fixed elements.
- Overlays, modals, and dropdowns.

Avoid arbitrary design decisions.

Use measurements from the reference website whenever possible.

**The original design takes priority over personal design preferences.**

## 2.2 Design Token Extraction

Extract reusable design tokens for:

- Colors.
- Typography.
- Spacing.
- Container widths.
- Breakpoints.
- Border radii.
- Shadows.
- Transitions.
- Z-index layers.
- Component variants.

Create a centralized design system instead of scattering hardcoded values throughout the project.

Reuse design tokens consistently across all pages.

## 2.3 Asset Reconstruction

Identify and reproduce authorized website assets:

- Logos and brand marks.
- Icons.
- Images and photography.
- Illustrations.
- Backgrounds.
- Fonts.
- SVG graphics.
- Favicons.
- Videos and other media.

Where source assets are available, reuse them rather than creating approximations.

Preserve original proportions, image quality, crop positioning, and responsive behavior.

Document any assets that cannot be obtained or legally reused. Never silently replace them with unrelated content.

---

# PHASE 3 - Responsive Design Reconstruction

The website must accurately reproduce the original behavior on all major viewport sizes.

## Desktop

Reconstruct:

- Full-width layouts.
- Navigation systems.
- Multi-column grids.
- Large typography.
- Hover interactions.
- Sticky elements.
- Desktop-specific components.

## Tablet

Reconstruct:

- Column adjustments.
- Tablet navigation.
- Spacing changes.
- Image scaling.
- Content rearrangement.
- Touch behavior.

## Mobile

Reconstruct:

- Mobile navigation and menus.
- Single-column layouts.
- Responsive images.
- Mobile typography and spacing.
- Sticky controls.
- Touch interactions.
- Mobile-specific animations and behaviors.

Do not simply force desktop elements into smaller widths.

Inspect how the reference website actually behaves at different screen sizes.

Test at representative widths including:

- 320px
- 375px
- 390px
- 768px
- 1024px
- 1440px
- 1920px

Also inspect intermediate widths to identify layout changes and actual breakpoints.

---

# PHASE 4 - Complete Component Recreation

Identify every unique visual and functional component.

Examples include:

- Headers and navigation menus.
- Footers.
- Hero sections.
- Buttons and links.
- Cards.
- Content sections.
- Accordions.
- Tabs.
- Carousels and sliders.
- Image galleries.
- Forms and inputs.
- Search bars.
- Filter panels.
- Dropdowns.
- Modal dialogs.
- Tooltips.
- Notifications.
- Breadcrumbs.
- Pagination.
- Data tables.
- Maps.
- Video players.
- Cookie consent interfaces.
- Authentication interfaces.
- Loading indicators.
- Empty and error states.

Implement each component as a reusable, configurable element wherever appropriate.

Avoid duplicating the same UI implementation across multiple pages.

Components should support variations without unnecessary complexity.

The result should preserve the original visual behavior while using a clean component architecture.

---

# PHASE 5 - Functional Reverse Engineering

## 5.1 Analyze Every Interaction

Do not stop after reproducing the visual appearance.

Inspect and implement the actual functionality of the reference website.

Test every relevant:

- Button.
- Navigation link.
- Dropdown.
- Form.
- Search field.
- Filter.
- Tab.
- Carousel.
- Modal.
- Interactive card.
- Accordion.
- Pagination control.
- Language selector.
- Authentication flow.
- Dynamic feature.

Document what happens when the user interacts with each element.

## 5.2 Reconstruct User Workflows

Analyze complete user journeys, including:

- Landing on the website.
- Navigating between pages.
- Finding information.
- Searching and filtering.
- Opening detail pages.
- Submitting forms.
- Receiving validation feedback.
- Interacting with account features.
- Completing key conversion actions.
- Handling empty or unsuccessful results.

Replicate observable behavior, not just the interface.

## 5.3 Backend & Data Functionality

Where the website uses backend services, determine the requirements for:

- APIs.
- Data persistence.
- Authentication and authorization.
- Content management.
- Database entities.
- Business logic.
- Search.
- Filtering and sorting.
- File uploads.
- User-generated content.
- External integrations.
- Email notifications.
- Form submissions.

If original backend source code and authorized access are available, analyze them directly.

Otherwise, reconstruct the required functionality from observable behavior and documented requirements.

Do not invent undocumented private APIs or pretend to have reproduced inaccessible functionality.

For third-party integrations, use authorized development credentials or sandbox environments.

**Important:** A button that only looks functional is not considered implemented.

Avoid fake interactions, meaningless redirects, hardcoded success messages, and placeholder business logic.

---

# PHASE 6 - Animation & Interaction Fidelity

Reproduce the original website's motion and interactive behavior.

Analyze:

- Page transitions.
- Hover animations.
- Scroll-triggered effects.
- Entrance animations.
- Fade transitions.
- Slide effects.
- Accordion animations.
- Dropdown transitions.
- Loading states.
- Carousels.
- Sticky header behavior.
- Smooth scrolling.
- Parallax effects.
- Interactive feedback.

Match the observed:

- Duration.
- Easing.
- Direction.
- Delay.
- Trigger.
- Scroll behavior.
- Element positioning.

Use lightweight, performant implementations.

Support reduced-motion preferences where appropriate without changing the normal visual experience.

Do not introduce additional animations that are absent from the original.

---

# PHASE 7 - Architecture & Code Quality

Although the website must visually and functionally match the reference, its internal implementation should follow modern software engineering standards.

## 7.1 Project Structure

Use a clear and scalable folder structure with separation between:

- Pages and routes.
- Layouts.
- Reusable components.
- Features.
- Styles and design tokens.
- Assets.
- API clients.
- Services.
- Data models and types.
- Utilities.
- Configuration.
- Tests.
- Documentation.

Follow the conventions of the selected framework instead of enforcing an unnecessarily complicated architecture.

## 7.2 Maintainability

Requirements:

- Type-safe code where supported.
- Reusable components.
- Consistent naming.
- Minimal duplication.
- No dead or unused code.
- No unnecessary dependencies.
- Separation of concerns.
- Centralized configuration.
- Well-defined component interfaces.
- Robust error handling.
- No hardcoded secrets.
- Appropriate logging.
- ESLint and formatting rules.
- Clear documentation.

Choose maintainable solutions over shortcuts.

## 7.3 Performance

Optimize without changing the visible design or intended behavior.

Consider:

- Image optimization.
- Lazy loading.
- Code splitting.
- Caching.
- Font loading.
- Reduced unnecessary JavaScript.
- Efficient rendering.
- Minimal layout shifts.
- Efficient API requests.
- Core Web Vitals.

## 7.4 Security, Accessibility & SEO

Implement:

- Secure input validation.
- Appropriate output encoding.
- Secure authentication and session handling.
- Correct authorization.
- Protection against common web vulnerabilities.
- Accessible keyboard interactions.
- Semantic HTML.
- Correct focus management.
- Responsive touch targets.
- WCAG 2.2 AA as a minimum practical accessibility target.
- SEO-friendly URLs.
- Correct titles and metadata.
- Canonical tags.
- Sitemap and robots directives where appropriate.
- Structured data when applicable.
- Open Graph metadata.

Research and consider emerging WCAG 3 guidance where useful, but do not treat it as a finalized conformance standard.

Preserve the reference website's intended functionality and presentation while improving implementation quality.

---

# PHASE 8 - Build Every Page

Implement every page identified during discovery.

For each page:

1. Recreate the exact page layout.
2. Add the authorized original content.
3. Implement all visual components.
4. Apply correct typography and spacing.
5. Reproduce original images and media.
6. Match the responsive layouts.
7. Implement interactions.
8. Connect required data and backend services.
9. Implement loading and error states.
10. Compare against the original.
11. Fix discrepancies.
12. Run functional tests.

Do not move on simply because a page renders successfully.

A completed page must be both visually and functionally validated.

Avoid building one generic page and reusing it for unrelated routes.

Use shared templates only where the reference website actually follows a shared structure.

---

# PHASE 9 - Automated Testing & Visual Comparison

## 9.1 Browser-Based Testing

Use Playwright or an equivalent browser automation framework.

Test the reconstructed website in real browsers.

Cover:

- Navigation.
- Page rendering.
- Forms.
- Interactive elements.
- Search and filters.
- State changes.
- Responsive layouts.
- Keyboard interactions.
- Functional workflows.
- Error handling.
- Console errors.
- Network failures.

## 9.2 Screenshot Comparison

Take screenshots of both the reference website and the local reconstruction.

Use identical:

- Viewport dimensions.
- Device scale factor.
- Browser environment.
- Scroll position.
- Page state.
- Font loading state.
- Animation state.

Compare the screenshots visually.

Perform comparisons for desktop, tablet, and mobile.

Check:

- Element positions.
- Spacing.
- Typography.
- Colors.
- Image sizes and crops.
- Alignment.
- Responsive differences.
- Section heights.
- Visual effects.

Where possible, use automated pixel-difference or perceptual image-difference testing.

Configure appropriate tolerances for browser rendering differences and dynamic content.

Do not fabricate similarity percentages.

## 9.3 Iterative Correction

Follow this loop:

**Inspect → Implement → Screenshot → Compare → Identify Differences → Fix → Retest**

Repeat until major differences have been resolved.

Prioritize:

1. Incorrect layouts and missing sections.
2. Missing or incorrect functionality.
3. Typography and asset mismatches.
4. Spacing and alignment differences.
5. Colors, borders, and shadows.
6. Animation and micro-interaction differences.

A page is not complete merely because its screenshot looks approximately correct.

## 9.4 Complete End-to-End Tests

Create end-to-end tests covering all major user journeys.

Include both successful and failure scenarios.

Verify that real actions cause the correct state changes.

Do not count mocked success responses as proof of working production integrations.

---

# PHASE 10 - Feature & Page Parity Audit

Create a traceable parity matrix.

For example:

| Page / Feature | Visual   | Responsive | Functional | E2E     | Status      |
| -------------- | -------- | ---------- | ---------- | ------- | ----------- |
| Homepage       | Verified | Verified   | Verified   | Passed  | Complete    |
| Navigation     | Verified | Verified   | Verified   | Passed  | Complete    |
| Contact form   | Verified | Verified   | Verified   | Passed  | Complete    |
| Search         | Pending  | Pending    | Pending    | Pending | In progress |

Include every discovered page and significant feature.

Use actual test results rather than assumed statuses.

Every finding must be classified as:

- Complete.
- Partially complete.
- Blocked.
- Not started.
- Not applicable.

Document why any feature cannot be replicated.

---

# PHASE 11 - Final Documentation

Create the following Markdown files:

### `docs/website-clone-analysis.md`

Complete original website analysis, page inventory, functionality, and architecture.

### `docs/website-design-system.md`

Extracted design tokens, typography, spacing, components, and responsive behavior.

### `docs/website-functional-specification.md`

Complete interaction requirements, feature descriptions, user journeys, and backend dependencies.

### `docs/website-clone-progress.md`

Implementation tracking, page completion, remaining work, and identified blockers.

### `docs/website-clone-testing.md`

Functional tests, visual comparisons, E2E results, and known discrepancies.

### `docs/website-clone-architecture.md`

Project structure, technical decisions, reusable components, data flow, APIs, and Mermaid diagrams.

Keep documentation synchronized with actual implementation progress.

---

# PHASE 12 - Final Verification

Before declaring the project complete, verify all of the following:

- Every discoverable in-scope page has been analyzed.
- Every required page has been implemented.
- All navigation paths are working.
- All authorized original content is represented.
- All required visual components exist.
- Desktop, tablet, and mobile layouts match the reference.
- Interactions reproduce observed behavior.
- Functional forms work correctly.
- Backend-dependent features work where necessary infrastructure and access are available.
- Important animations are reproduced.
- Screenshots have been compared.
- Critical visual differences have been resolved.
- Automated tests pass.
- No critical runtime errors remain.
- No unresolved build or lint errors remain.
- No obvious accessibility regressions remain.
- Performance has been evaluated.
- Security controls have been reviewed.
- Documentation is complete.
- Remaining limitations are explicitly identified.

If any requirement cannot be satisfied, report the precise reason instead of marking the website as fully cloned.

---

# CRITICAL RULES

1. **No redesign:** Do not modernize, reinterpret, or visually improve the reference design unless explicitly instructed.

2. **No partial clone:** Do not stop after building the homepage or primary navigation. Cover the entire discovered website scope.

3. **No placeholder functionality:** Interactive elements must behave correctly.

4. **No invented content:** Preserve the actual content where authorized, rather than generating generic replacement text.

5. **No design guessing:** Inspect the original before implementing visual elements.

6. **No unnecessary rewrites:** Use clean architecture and reusable components without overengineering.

7. **No fake completion:** Only mark features complete when they have been verified.

8. **No security shortcuts:** Do not copy real user sessions, private data, passwords, payment credentials, or production secrets. Use authorized test accounts and environments.

9. **No functionality loss:** Preserve identified business-critical functionality and user journeys.

10. **No silent omissions:** Document inaccessible pages, unavailable assets, unknown behaviors, and blocked integrations.

11. **Continuous verification:** Compare the reconstruction with the reference throughout implementation, not only at the end.

12. **Keep working through the scope:** Continue systematically until the discovered pages and features are implemented and verified, or a genuine technical/access blocker is documented.

---

# DEFINITION OF DONE

The project is considered complete only when:

**Visual fidelity:** The recreated pages closely match the original in layout, typography, colors, assets, spacing, and responsiveness.

**Functional fidelity:** The website's required interactions and user workflows function correctly, with backend behavior implemented and tested where access and requirements permit.

**Content fidelity:** All in-scope pages accurately represent the original authorized content.

**Technical quality:** The implementation is maintainable, scalable, secure, accessible, and follows the selected framework's best practices.

**Test coverage:** All major pages and workflows have been validated with browser-based tests and visual comparisons.

**Documentation:** Every relevant page, feature, architecture decision, limitation, and remaining discrepancy is documented.

## FINAL EXPECTED RESULT

Deliver a complete, production-ready reconstruction of the authorized reference website.

The user should be able to navigate the recreated website and experience essentially the same design, content, responsive behavior, animations, and functionality as the original.

The objective is not merely to create something similar.

**The objective is maximum achievable 1:1 visual and functional fidelity, supported by clean architecture, real functionality, comprehensive testing, and accurate documentation.**