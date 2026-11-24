(() => {
  if (document.querySelector('input[type=password]')) return null;
  const root=document.querySelector('main,article,[role=main]') || document.body;
  if (!root) return null;
  const headings=[];
  for (const h of root.querySelectorAll('h1,h2')) {
    if (headings.length===6) break;
    if (h.checkVisibility({checkVisibilityCSS:true}) && !h.closest('form,[contenteditable=true]')) {
      const text=h.innerText.trim().slice(0,100);
      if (text) headings.push(text);
    }
  }
  return {href:location.href,title:document.title.slice(0,160),headings};
})()
