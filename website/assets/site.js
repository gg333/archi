// Archi website — keeps the version label and download links current.
// Fetches the latest release from the public GitHub API; on any failure the
// static "0.2.0" markup stays as-is and links keep pointing at the releases page.
(function () {
  "use strict";

  var header = document.getElementById("site-header");
  var onScroll = function () {
    if (!header) return;
    header.classList.toggle("is-pinned", window.scrollY > 4);
  };
  window.addEventListener("scroll", onScroll, { passive: true });
  onScroll();

  if (!window.fetch) return;

  fetch("https://api.github.com/repos/gg333/archi/releases/latest")
    .then(function (response) {
      if (!response.ok) throw new Error("HTTP " + response.status);
      return response.json();
    })
    .then(function (release) {
      var version = (release.tag_name || "").replace(/^v/, "");
      if (!version || !/^\d[\w.-]*$/.test(version)) return;

      var labels = document.querySelectorAll("[data-version]");
      for (var i = 0; i < labels.length; i++) {
        labels[i].textContent = version;
      }

      // The DMG asset name embeds the version, e.g. Archi_0.2.0_universal.dmg.
      var asset = (release.assets || []).filter(function (file) {
        return /^Archi_.+_universal\.dmg$/.test(file.name || "");
      })[0];
      if (!asset || !asset.browser_download_url) return;

      var links = document.querySelectorAll("[data-download-link]");
      for (var j = 0; j < links.length; j++) {
        links[j].href = asset.browser_download_url;
      }
    })
    .catch(function () {
      /* offline or rate-limited: static fallbacks already in place */
    });
})();
