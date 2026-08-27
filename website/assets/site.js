(function () {
  "use strict";

  var header = document.querySelector(".site-header");
  var menu = document.getElementById("site-nav");
  var toggle = document.querySelector(".menu-toggle");

  function closeMenu() {
    if (!menu || !toggle) return;
    menu.removeAttribute("data-open");
    toggle.setAttribute("aria-expanded", "false");
    toggle.setAttribute("aria-label", "Open navigation menu");
  }

  window.addEventListener("scroll", function () {
    if (header) header.classList.toggle("is-pinned", window.scrollY > 4);
  }, { passive: true });

  if (menu && toggle) {
    toggle.addEventListener("click", function () {
      var open = !menu.hasAttribute("data-open");
      menu.toggleAttribute("data-open", open);
      toggle.setAttribute("aria-expanded", String(open));
      toggle.setAttribute("aria-label", open ? "Close navigation menu" : "Open navigation menu");
    });
    menu.addEventListener("click", closeMenu);
    document.addEventListener("keydown", function (event) {
      if (event.key === "Escape") closeMenu();
    });
  }
})();
