// Business Charts 6.6: responsive wind compass, north fixed at the top.
// The arrow points to the station's bearing, including 0 degrees at zero speed.
const latest = (name) => {
  for (const frame of context.panel.data.series) {
    const field = frame.fields.find((item) => item.name === name);
    if (!field) continue;
    const value = field.values[field.values.length - 1];
    if (value !== null && value !== undefined && Number.isFinite(Number(value))) return Number(value);
  }
  return null;
};
const speed = latest('wind_speed_ms');
const rawDirection = latest('wind_direction_deg');
const direction = rawDirection === null ? null : ((rawDirection % 360) + 360) % 360;
const width = context.panel.chart.getWidth();
const height = context.panel.chart.getHeight();
const radius = Math.max(12, Math.min(width / 2.7, height / 3.15));
const cx = width / 2;
const cy = height / 2 - radius * 0.10;
const dark = context.grafana.theme.isDark;
const foreground = dark ? '#f0f7ff' : '#163651';
const muted = dark ? '#91b9dc' : '#406888';
const dial = dark ? '#172e45' : '#e7f2fb';
const elements = [];
const line = (x1, y1, x2, y2, stroke, lineWidth) => ({
  type: 'line', silent: true, shape: { x1, y1, x2, y2 }, style: { stroke, lineWidth, lineCap: 'round' },
});
const text = (x, y, value, size, color, weight = 600) => ({
  type: 'text', silent: true, style: {
    x, y, text: value, fill: color, font: `${weight} ${size}px sans-serif`, align: 'center', verticalAlign: 'middle',
  },
});
elements.push({ type: 'circle', silent: true, shape: { cx, cy, r: radius }, style: { fill: dial, stroke: muted, lineWidth: 1, opacity: 0.65 } });
for (let tick = 0; tick < 72; tick++) {
  const angle = tick * Math.PI / 36;
  const major = tick % 18 === 0;
  const inner = radius * (major ? 0.84 : tick % 3 === 0 ? 0.89 : 0.94);
  elements.push(line(cx + inner * Math.sin(angle), cy - inner * Math.cos(angle),
    cx + radius * Math.sin(angle), cy - radius * Math.cos(angle), muted, major ? 2.5 : 1));
}
const labelSize = Math.max(12, Math.min(26, radius * 0.18));
for (const [label, x, y] of [['С', 0, -1.17], ['В', 1.17, 0], ['Ю', 0, 1.17], ['З', -1.17, 0]]) {
  elements.push(text(cx + x * radius, cy + y * radius, label, labelSize, foreground));
}
if (direction !== null) {
  const angle = direction * Math.PI / 180;
  const dx = Math.sin(angle), dy = -Math.cos(angle);
  const tipX = cx + dx * radius * 0.78, tipY = cy + dy * radius * 0.78;
  const tailX = cx - dx * radius * 0.78, tailY = cy - dy * radius * 0.78;
  elements.push(line(tailX, tailY, tipX, tipY, foreground, Math.max(2, radius * 0.025)));
  const backX = tipX - dx * radius * 0.13, backY = tipY - dy * radius * 0.13;
  elements.push({ type: 'polygon', silent: true, shape: { points: [
    [tipX, tipY], [backX - dy * radius * 0.065, backY + dx * radius * 0.065],
    [backX + dy * radius * 0.065, backY - dx * radius * 0.065],
  ] }, style: { fill: foreground } });
  elements.push({ type: 'circle', silent: true, shape: { cx: tailX, cy: tailY, r: radius * 0.045 }, style: { fill: foreground } });
}
elements.push({ type: 'circle', silent: true, shape: { cx, cy, r: radius * 0.34 }, style: { fill: dial } });
const numberSize = Math.max(15, Math.min(48, radius * 0.30));
const speedText = speed === null ? '—' : speed.toLocaleString('ru-RU', { maximumFractionDigits: 1 });
elements.push(text(cx, cy - radius * 0.07, speedText, numberSize, foreground, 700));
elements.push(text(cx, cy + radius * 0.17, 'м/с', Math.max(10, numberSize * 0.55), foreground));
elements.push(text(cx, cy + radius * 1.47, direction === null ? 'Нет направления' : `${Math.round(direction) % 360}°`, labelSize, muted));
return { animation: false, backgroundColor: 'transparent', tooltip: { show: false }, graphic: elements };
