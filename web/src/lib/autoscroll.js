export function autoscroll(node) {
  const scroll = () => (node.scrollTop = node.scrollHeight);
  scroll();
  return { update: scroll };
}
