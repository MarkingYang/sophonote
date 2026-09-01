const REPOSITORY = "MarkingYang/sophonote";
const RELEASES_URL = `https://github.com/${REPOSITORY}/releases`;
const RELEASES_API_URL = `https://api.github.com/repos/${REPOSITORY}/releases?per_page=10`;

const releaseLinks = [...document.querySelectorAll("[data-release-link]")];
const releaseStatuses = [...document.querySelectorAll("[data-release-status]")];

function formatBytes(bytes) {
  if (!Number.isFinite(bytes) || bytes <= 0) return null;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

function formatDate(value) {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return null;
  return new Intl.DateTimeFormat("zh-CN", {
    year: "numeric",
    month: "short",
    day: "numeric",
  }).format(date);
}

function chooseDmg(assets) {
  const dmgAssets = assets.filter((asset) => asset.name?.toLowerCase().endsWith(".dmg"));
  return (
    dmgAssets.find((asset) => /(aarch64|arm64|apple[-_ ]?silicon)/i.test(asset.name)) ??
    dmgAssets[0]
  );
}

function setReleaseFallback(message) {
  releaseLinks.forEach((link) => {
    link.href = RELEASES_URL;
    link.removeAttribute("download");
  });
  releaseStatuses.forEach((status) => {
    status.textContent = message;
  });
}

async function hydrateLatestRelease() {
  try {
    const response = await fetch(RELEASES_API_URL, {
      headers: { Accept: "application/vnd.github+json" },
    });
    if (!response.ok) throw new Error(`GitHub API returned ${response.status}`);

    const releases = await response.json();
    const release = releases.find((candidate) => !candidate.draft && chooseDmg(candidate.assets ?? []));
    const dmg = release ? chooseDmg(release.assets ?? []) : null;
    if (!release || !dmg?.browser_download_url) {
      setReleaseFallback("最新 Release 暂无 DMG；可先从源码运行，或关注 GitHub Releases。");
      return;
    }

    releaseLinks.forEach((link) => {
      link.href = dmg.browser_download_url;
      link.setAttribute("download", "");
    });

    const details = [
      release.tag_name,
      formatBytes(dmg.size),
      formatDate(release.published_at),
      /(aarch64|arm64|apple[-_ ]?silicon)/i.test(dmg.name) ? "Apple Silicon" : null,
    ].filter(Boolean);
    releaseStatuses.forEach((status) => {
      const channel = release.prerelease ? "GitHub Community Preview" : "GitHub Release";
      status.textContent = `${channel} · ${details.join(" · ")}`;
    });
  } catch (error) {
    console.info("Latest SophoNote release is not available yet.", error);
    setReleaseFallback("GitHub 暂无可读取的公开 DMG；可先从源码运行，或关注 Releases。");
  }
}

function setupRevealAnimations() {
  const elements = [...document.querySelectorAll(".reveal")];
  if (!("IntersectionObserver" in window)) {
    elements.forEach((element) => element.classList.add("visible"));
    return;
  }

  const observer = new IntersectionObserver(
    (entries) => {
      entries.forEach((entry) => {
        if (!entry.isIntersecting) return;
        entry.target.classList.add("visible");
        observer.unobserve(entry.target);
      });
    },
    { rootMargin: "0px 0px -8%", threshold: 0.12 },
  );
  elements.forEach((element) => observer.observe(element));
}

function setupScrollState() {
  const header = document.querySelector("#site-header");
  const progress = document.querySelector("#page-progress");
  const update = () => {
    const scrollable = document.documentElement.scrollHeight - window.innerHeight;
    const ratio = scrollable > 0 ? window.scrollY / scrollable : 0;
    header?.classList.toggle("scrolled", window.scrollY > 24);
    if (progress) progress.style.transform = `scaleX(${Math.min(Math.max(ratio, 0), 1)})`;
  };
  update();
  window.addEventListener("scroll", update, { passive: true });
  window.addEventListener("resize", update, { passive: true });
}

setupRevealAnimations();
setupScrollState();
hydrateLatestRelease();
