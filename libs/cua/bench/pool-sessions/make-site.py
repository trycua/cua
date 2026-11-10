#!/usr/bin/env python3
"""make-site.py OUTDIR — build a self-contained local test site (p0..p9.html) from 10 real
Wikipedia articles (CC BY-SA): the article HTML, its stylesheet and up to 8 images per page are
downloaded ONCE here and served locally, so benchmark sessions never load third-party sites."""
import os, re, sys, urllib.request, hashlib

OUT = sys.argv[1] if len(sys.argv) > 1 else os.path.expanduser("~/fb/site")
os.makedirs(os.path.join(OUT, "a"), exist_ok=True)
UA = {"User-Agent": "cua-fleet-bench/1.0 (one-time test fixture download)"}
ARTICLES = ["Web_scraping", "Headless_browser", "Chromium_(web_browser)", "Virtual_machine",
            "Hypervisor", "Kubernetes", "Docker_(software)", "Startup_company",
            "Venture_capital", "E-commerce"]

def get(url):
    return urllib.request.urlopen(urllib.request.Request(url, headers=UA), timeout=30).read()

def local(url):
    if url.startswith("//"):
        url = "https:" + url
    elif url.startswith("/"):
        url = "https://en.wikipedia.org" + url
    name = hashlib.sha1(url.encode()).hexdigest()[:16] + os.path.splitext(url.split("?")[0])[1][:5]
    path = os.path.join(OUT, "a", name)
    if not os.path.exists(path):
        try:
            data = get(url)
        except Exception:
            return None
        open(path, "wb").write(data)
    return "a/" + name

total = 0
for i, art in enumerate(ARTICLES):
    html = get("https://en.wikipedia.org/wiki/" + art).decode("utf-8", "replace")
    # stylesheets: keep the first load.php CSS locally, drop the rest
    css_done = False
    def css(m):
        global css_done
        if css_done or "stylesheet" not in m.group(0):
            return ""
        href = re.search(r'href="([^"]+)"', m.group(0))
        p = local(href.group(1).replace("&amp;", "&")) if href else None
        css_done = p is not None
        return f'<link rel="stylesheet" href="{p}">' if p else ""
    html = re.sub(r"<link[^>]+>", css, html)
    html = re.sub(r"<script[^>]*src=[^>]*></script>", "", html)
    n_img = 0
    def img(m):
        global n_img
        src = m.group(1)
        if n_img >= 8:
            return 'src="data:,"'
        p = local(src)
        if p:
            n_img += 1
            return f'src="{p}"'
        return 'src="data:,"'
    html = re.sub(r'src="([^"]+\.(?:png|jpg|jpeg|svg|gif|webp)[^"]*)"', img, html)
    html = re.sub(r'srcset="[^"]*"', "", html)
    html = re.sub(r'(href)="(?:https?:)?//[^"]*"', r'\1="#"', html)
    # a little client-side work, like real sites: build and measure a DOM list
    html = html.replace("</body>", "<script>let t=performance.now();let u=document.createElement('ul');"
                        "for(let k=0;k<2000;k++){let li=document.createElement('li');li.textContent='item '+k;u.appendChild(li)}"
                        "document.body.appendChild(u);document.title+=' '+(performance.now()-t).toFixed(1)+'ms';</script></body>")
    open(os.path.join(OUT, f"p{i}.html"), "w").write(html)
    total += len(html)
    print(f"p{i}.html {art} {len(html)//1024} KiB images={n_img}")
print("total html KiB", total // 1024)
