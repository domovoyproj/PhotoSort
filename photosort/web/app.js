const $ = (selector) => document.querySelector(selector);
const titles = {
  all: "Все фотографии",
  favorites: "Избранное",
  similar: "Похожие кадры",
  bursts: "Серии",
  duplicates: "Точные дубли",
  trash: "Корзина",
};
const state = {
  view: "all",
  offset: 0,
  photos: [],
  selected: new Set(),
  token: "",
  running: false,
  request: 0,
  busy: false,
  culling: false,
  viewerPhotos: [],
  viewerIndex: 0,
  settings: {
    sensitivity: 6,
    cache_mb: 1024,
    preferred_folder: "",
    theme: "light",
    density: "normal",
    ui_state: {},
  },
};
const number = (value) => Number(value || 0).toLocaleString("ru-RU");
const size = (value) =>
  value >= 1e9
    ? `${(value / 1e9).toFixed(1)} ГБ`
    : value >= 1e6
      ? `${(value / 1e6).toFixed(1)} МБ`
      : `${Math.max(1, Math.round(value / 1024))} КБ`;
let toastTimer,
  searchTimer,
  saveTimer,
  eyeWorker,
  eyeQueue = [],
  zoom = 1,
  pan = { x: 0, y: 0 },
  drag = null,
  pixel = false;
function element(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}
function toast(message) {
  $("#toast").textContent = message;
  $("#toast").hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => ($("#toast").hidden = true), 6500);
}
async function api(path, body) {
  const response = await fetch(
    path,
    body === undefined
      ? {}
      : {
          method: "POST",
          headers: {
            "Content-Type": "application/json",
            "X-PhotoSort-Token": state.token,
          },
          body: JSON.stringify(body),
        },
  );
  const data = await response.json();
  if (!response.ok) throw Error(data.error || "Ошибка запроса");
  return data;
}
async function action(work) {
  if (state.busy) return;
  state.busy = true;
  document.body.classList.add("busy");
  try {
    return await work();
  } catch (error) {
    toast(error.message);
  } finally {
    state.busy = false;
    document.body.classList.remove("busy");
  }
}
function outcome(result, label) {
  toast(
    `${label}: ${Array.isArray(result.done) ? result.done.length : result.done}${result.errors?.length ? `. Ошибок: ${result.errors.length}. ${result.errors[0].message}` : ""}`,
  );
}
function applyTheme() {
  const theme = state.settings.theme;
  document.body.classList.toggle(
    "theme-dark",
    theme === "dark" ||
      (theme === "system" && matchMedia("(prefers-color-scheme:dark)").matches),
  );
  $("#gallery").classList.toggle("large", state.settings.density === "large");
}
async function status() {
  const info = await api("/api/status");
  state.settings = info.settings;
  applyTheme();
  $("#undo").disabled = !info.undo;
  $("#undo").title = info.undo
    ? `Отменить: ${info.undo} · Ctrl+Z`
    : "Нет действий для отмены";
  $("#codec-note").textContent = info.codecs
    ? "JPEG · PNG · TIFF · WEBP · HEIC · RAW. RAW+JPEG выбираются и экспортируются вместе."
    : "Для HEIC и RAW требуется комплект локальных декодеров из полной сборки.";
  progress(info.progress);
  return info;
}
function persist() {
  clearTimeout(saveTimer);
  saveTimer = setTimeout(async () => {
    try {
      state.settings.ui_state = {
        view: state.view,
        offset: state.offset,
        sort: $("#sort").value,
        search: $("#search").value,
        cullKind: state.cullKind || "similar",
      };
      await api("/api/settings", state.settings);
    } catch (error) {
      toast(`Не удалось сохранить место разбора: ${error.message}`);
    }
  }, 450);
}
function updateSelection() {
  const n = state.selected.size;
  $("#selection-bar").hidden = !n;
  $("#selection-count").textContent = `Выбрано: ${n}`;
  $("#trash-selected").textContent =
    state.view === "trash" ? "Восстановить" : "В корзину";
  $("#compare-selected").disabled = n !== 2;
  $("#favorite-selected").textContent =
    state.view === "favorites" ? "♡ Убрать из избранного" : "♡ В избранное";
  document.querySelectorAll(".photo-card").forEach((card) => {
    const selected = state.selected.has(Number(card.dataset.id));
    card.classList.toggle("selected", selected);
    card.querySelector("input").checked = selected;
  });
}
function visibleAssets(photos) {
  const groups = new Map();
  for (const p of photos) {
    const key = p.asset_group || p.path;
    if (!groups.has(key)) groups.set(key, []);
    groups.get(key).push(p);
  }
  const result = [];
  for (const group of groups.values()) {
    const raw = group.some((p) =>
      /\.(cr2|cr3|nef|nrw|arw|dng|raf|rw2|orf|pef|srw|raw)$/i.test(p.path),
    );
    if (raw && group.length > 1) {
      const p = group.find((p) => /\.jpe?g$/i.test(p.path)) || group[0];
      result.push({ ...p, pairCount: group.length });
    } else result.push(...group);
  }
  return result;
}
function card(photo, index) {
  const item = element("article", "photo-card");
  item.dataset.id = photo.id;
  const frame = element("div", "photo-image"),
    open = element("button", "photo-open");
  open.title = `Открыть ${photo.name}`;
  open.setAttribute("aria-label", open.title);
  const image = element("img");
  image.src = `/media/${photo.id}`;
  image.alt = photo.name;
  image.loading = "lazy";
  open.append(image);
  open.onclick = () => {
    state.culling = false;
    state.viewerPhotos = state.displayPhotos;
    showPhotos([photo], index);
  };
  frame.append(open);
  const check = element("input", "photo-select");
  check.type = "checkbox";
  check.setAttribute("aria-label", `Выбрать ${photo.name}`);
  check.onchange = () => {
    check.checked
      ? state.selected.add(photo.id)
      : state.selected.delete(photo.id);
    updateSelection();
  };
  frame.append(check);
  const heart = element("button", "photo-heart", photo.favorite ? "♥" : "♡");
  heart.title = photo.favorite ? "Убрать из избранного" : "В избранное";
  heart.setAttribute("aria-label", heart.title);
  heart.onclick = () =>
    action(async () => {
      await api("/api/favorite", { ids: [photo.id], value: !photo.favorite });
      await load();
      await status();
    });
  frame.append(heart);
  const badge = photo.pairCount
    ? `RAW + JPEG · ${photo.pairCount}`
    : {
        duplicates: `Дубль · ${photo.hash.slice(0, 6)}`,
        similar: `Группа ${photo.similar}`,
        bursts: `Серия ${photo.burst}`,
        trash: "В корзине",
      }[state.view];
  if (badge) frame.append(element("span", "photo-badge", badge));
  const caption = element("div", "photo-caption");
  caption.append(
    element("strong", "", photo.name),
    element("span", "", size(photo.size)),
  );
  const date = photo.captured
    ? new Date(photo.captured * 1000).toLocaleDateString("ru-RU")
    : "Без даты съёмки";
  item.append(
    frame,
    caption,
    element(
      "div",
      "photo-detail",
      `${photo.width} × ${photo.height} · ${date}`,
    ),
  );
  const hints = [
    photo.quality_hint,
    photo.faces > 0 && photo.eye_score > 0.65 ? "Возможно, закрыты глаза" : "",
  ].filter(Boolean);
  if (hints.length)
    item.append(element("div", "quality-hint", hints.join(" · ")));
  if (photo.decision !== "pending")
    item.append(
      element(
        "span",
        "decision-badge",
        { keep: "✓ Оставить", reject: "× Отклонён", skip: "→ Просмотрен" }[
          photo.decision
        ] || "",
      ),
    );
  return item;
}
async function load() {
  const request = ++state.request;
  try {
    const query = new URLSearchParams({
      view: state.view,
      offset: state.offset,
      search: $("#search").value,
      sort: $("#sort").value,
    });
    const data = await api(`/api/photos?${query}`);
    if (request !== state.request) return;
    if (!data.photos.length && state.offset > 0) {
      state.offset = Math.max(0, state.offset - 80);
      return load();
    }
    state.photos = data.photos;
    state.displayPhotos = visibleAssets(data.photos);
    for (const key of Object.keys(titles))
      $(`#count-${key}`).textContent = number(data.counts[key]);
    $("#stat-total").replaceChildren(
      document.createTextNode(number(data.counts.all) + " "),
      element("small", "", "фото"),
    );
    $("#stat-similar").textContent = number(data.counts.similar);
    $("#stat-duplicates").textContent = number(data.counts.duplicates);
    $("#stat-favorites").replaceChildren(
      document.createTextNode(number(data.counts.favorites) + " "),
      element("span", "", "♡"),
    );
    $("#stat-bytes").textContent = `${size(data.counts.bytes)} на диске`;
    $("#total-label").textContent = `${number(data.total)} файлов`;
    $("#gallery").replaceChildren(...state.displayPhotos.map(card));
    $("#empty").hidden = data.total !== 0;
    const first =
      state.view === "all" && !$("#search").value && !data.counts.all;
    $("#empty-title").textContent = first
      ? "Всё начинается с одной папки"
      : state.view === "trash"
        ? "Корзина пуста"
        : "Здесь пока нет фотографий";
    $("#empty-description").textContent = first
      ? "Добавьте фотографии. PhotoSort найдёт похожие кадры, объединит серии и поможет оставить самое ценное."
      : state.view === "trash"
        ? "Перемещённые сюда фотографии можно восстановить в исходную папку."
        : "Попробуйте другой поиск или добавьте фотографии в библиотеку.";
    $("#empty-import").hidden = !first;
    $("#previous").disabled = state.offset === 0;
    $("#next").disabled = state.offset + 80 >= data.total;
    $("#page-number").textContent = String(
      Math.floor(state.offset / 80) + 1,
    ).padStart(2, "0");
    $("#footer-text").textContent = data.total
      ? `${state.offset + 1}–${Math.min(state.offset + 80, data.total)} из ${number(data.total)} файлов`
      : "Порядок начинается здесь";
    $("#review-progress").textContent = data.counts.all
      ? `Осталось разобрать: ${number(data.counts.pending)}`
      : "";
    progress(data.progress);
    updateSelection();
  } catch (error) {
    toast(`Не удалось загрузить библиотеку: ${error.message}`);
  }
}
function progress(data) {
  state.running = data.running;
  $("#scan-status").hidden = !data.running && !data.paused;
  $("#scan-text").textContent =
    `${data.paused ? "На паузе · " : ""}Обработано ${number(data.done)}${data.total ? ` из ${number(data.total)}` : ""} фото`;
  $("#scan-detail").textContent =
    `${data.message}${data.errors ? ` · ошибок: ${data.errors}` : ""}`;
  $("#stop-scan").hidden = !data.running;
  $("#resume-scan").hidden = !data.paused;
  $("#scan-status").classList.toggle("paused", data.paused);
}
function setView(viewName, save = true) {
  state.view = viewName;
  state.offset = 0;
  state.selected.clear();
  document
    .querySelectorAll("[data-view]")
    .forEach((b) => b.classList.toggle("active", b.dataset.view === viewName));
  $("#page-title").replaceChildren(
    document.createTextNode(titles[viewName]),
    element("span", "title-dot", "."),
  );
  $("#crumb").textContent = titles[viewName];
  $("#collection-name").textContent =
    viewName === "all" ? "Коллекция" : titles[viewName];
  const hints = {
    similar:
      "Визуальное сходство включает перекодирование и небольшую обрезку. Чувствительность можно изменить в настройках. Проверяйте важные детали при сравнении.",
    bursts:
      "Кадры из одной папки с интервалом до 8 секунд по EXIF. RAW+JPEG выбираются вместе.",
    duplicates:
      "Точные дубли совпадают по SHA-256. «Разобрать дубли» предложит сохраняемые копии с учётом избранного и предпочтительной папки.",
    trash:
      "Выделите кадры для восстановления. Автоматической очистки нет. Ctrl+Z отменяет последнее действие.",
  };
  $("#view-hint").hidden = !hints[viewName];
  $("#view-hint").textContent = hints[viewName] || "";
  load();
  if (save) persist();
}
function applyZoom(stage = null) {
  for (const node of document.querySelectorAll(".zoom-stage img")) {
    if (stage && !$("#sync-zoom").checked && node.parentElement !== stage)
      continue;
    const magnification = pixel
      ? node.naturalWidth / Math.max(node.clientWidth, 1)
      : zoom;
    node.style.transform = `translate(${pan.x * 100}%,${pan.y * 100}%) scale(${magnification})`;
  }
  $("#zoom-label").textContent = pixel ? "1:1" : `${Math.round(zoom * 100)}%`;
}
function resetZoom() {
  zoom = 1;
  pixel = false;
  pan = { x: 0, y: 0 };
  $("#zoom").value = "100";
  applyZoom();
}
function stageFor(photo) {
  const figure = element("figure"),
    stage = element("div", "zoom-stage"),
    image = element("img");
  image.src = `/media/${photo.id}?full=1`;
  image.alt = photo.name;
  image.draggable = false;
  image.onload = () => applyZoom();
  stage.append(image);
  stage.onwheel = (event) => {
    event.preventDefault();
    pixel = false;
    zoom = Math.max(1, Math.min(5, zoom * (event.deltaY < 0 ? 1.13 : 0.88)));
    $("#zoom").value = String(Math.round(zoom * 100));
    applyZoom(stage);
  };
  stage.onpointerdown = (event) => {
    drag = { x: event.clientX, y: event.clientY, pan: { ...pan }, stage };
    stage.setPointerCapture(event.pointerId);
  };
  stage.onpointermove = (event) => {
    if (drag?.stage !== stage) return;
    pan = {
      x: Math.max(
        -2,
        Math.min(2, drag.pan.x + (event.clientX - drag.x) / stage.clientWidth),
      ),
      y: Math.max(
        -2,
        Math.min(2, drag.pan.y + (event.clientY - drag.y) / stage.clientHeight),
      ),
    };
    applyZoom(stage);
  };
  stage.onpointerup = () => (drag = null);
  stage.onpointercancel = () => (drag = null);
  figure.append(
    stage,
    element(
      "figcaption",
      "",
      `${photo.name} · ${photo.width} × ${photo.height} · резкость ${Math.round(photo.sharpness)}${photo.quality_hint ? " · " + photo.quality_hint : ""}${photo.faces > 0 && photo.eye_score > 0.65 ? " · Возможно, закрыты глаза" : ""}`,
    ),
  );
  if (state.comparing) {
    const button = element("button", "button secondary", "♡ Оставить этот");
    button.onclick = () =>
      action(async () => {
        await api("/api/decision", { ids: [photo.id], decision: "keep" });
        await load();
        await status();
        toast(`Сохранён выбор: ${photo.name}`);
      });
    figure.append(button);
  }
  return figure;
}
function showPhotos(photos, index = 0) {
  state.viewerIndex = index;
  state.comparing = photos.length === 2;
  state.openPhotos = photos;
  $("#viewer-title").textContent = state.culling
    ? `Выбор лучшего · ${photos[0].name}`
    : state.comparing
      ? "Сравнение кадров"
      : photos[0].name;
  $("#viewer-images").replaceChildren(...photos.map(stageFor));
  $("#viewer-favorite").hidden = state.comparing || state.culling;
  $("#viewer-favorite").textContent = photos[0].favorite
    ? "♥ В избранном"
    : "♡ В избранное";
  $("#viewer-prev").disabled = state.comparing || index === 0;
  $("#viewer-next").disabled =
    state.comparing || index >= state.viewerPhotos.length - 1;
  $("#cull-controls").hidden = !state.culling;
  $("#filmstrip").hidden = !state.culling;
  resetZoom();
  if (state.culling) {
    $("#filmstrip").replaceChildren(
      ...state.viewerPhotos.map((p, i) => {
        const button = element(
          "button",
          i === index ? "film-frame selected" : "film-frame",
        );
        const img = element("img");
        img.src = `/media/${p.id}`;
        img.alt = p.name;
        button.title = p.name;
        button.append(img, element("span", "", String(i + 1)));
        button.onclick = () => showPhotos([p], i);
        return button;
      }),
    );
    $("#group-progress").textContent =
      `Групп осталось: ${state.group.remaining} из ${state.group.total}`;
  }
  if (!$("#viewer").open) $("#viewer").showModal();
}
async function nextGroup() {
  const data = await api(`/api/group?kind=${state.cullKind}`);
  state.group = data;
  if (!data.photos.length) {
    $("#viewer").close();
    state.culling = false;
    toast("Все группы просмотрены. Решения сохранены.");
    return;
  }
  state.culling = true;
  state.viewerPhotos = visibleAssets(data.photos);
  showPhotos([state.viewerPhotos[0]], 0);
  persist();
}
async function choose(skip = false) {
  return action(async () => {
    const result = await api("/api/choose", {
      ids: state.group.photos.map((p) => p.id),
      winner: skip ? null : state.viewerPhotos[state.viewerIndex].id,
      trash_others: $("#trash-others").checked,
    });
    if (result.errors?.length) outcome(result, "Выбор сохранён");
    await nextGroup();
    await load();
    await status();
  });
}
async function decide(decision) {
  const p = state.viewerPhotos[state.viewerIndex];
  return action(async () => {
    await api("/api/decision", {
      ids: $("#viewer").open ? [p.id] : [...state.selected],
      decision,
    });
    if ($("#viewer").open && state.viewerIndex < state.viewerPhotos.length - 1)
      showPhotos(
        [state.viewerPhotos[state.viewerIndex + 1]],
        state.viewerIndex + 1,
      );
    state.selected.clear();
    await load();
    await status();
  });
}
async function undo() {
  return action(async () => {
    const result = await api("/api/undo", {});
    outcome(result, "Отменено");
    await load();
    await status();
    if (state.culling) await nextGroup();
  });
}
async function doMove(restore) {
  return action(async () => {
    $("#confirm-dialog").close();
    const result = await api(restore ? "/api/restore" : "/api/trash", {
      ids: [...state.selected],
    });
    state.selected.clear();
    await load();
    await status();
    outcome(result, restore ? "Восстановлено" : "В корзине");
  });
}
async function pick(input) {
  return action(async () => {
    const result = await api("/api/pick", {});
    if (result.path) $(input).value = result.path;
  });
}
async function analyzeEyes() {
  if (eyeQueue.length) {
    toast("Проверка лиц уже выполняется");
    return;
  }
  eyeQueue = (
    state.selected.size
      ? state.photos.filter((p) => state.selected.has(p.id))
      : state.displayPhotos
  ).map((p) => p.id);
  if (!eyeQueue.length) {
    toast("Выберите фотографии или откройте страницу с кадрами");
    return;
  }
  if (!eyeWorker) {
    eyeWorker = new Worker("/face-worker.js");
    eyeWorker.onmessage = async ({ data }) => {
      if (data.error) {
        eyeQueue = [];
        $("#check-eyes").disabled = false;
        toast(`Проверка лиц недоступна: ${data.error}`);
        return;
      }
      try {
        await api("/api/eyes", {
          id: data.id,
          faces: data.faces,
          score: data.score,
        });
        eyeQueue.shift();
        if (eyeQueue.length) {
          $("#check-eyes").textContent = `Проверка лиц · ${eyeQueue.length}`;
          eyeWorker.postMessage({ id: eyeQueue[0] });
        } else {
          $("#check-eyes").disabled = false;
          $("#check-eyes").textContent = "Проверить лица";
          await load();
          toast(
            "Локальная проверка лиц завершена. Результаты — подсказки, проверьте их визуально.",
          );
        }
      } catch (error) {
        eyeQueue = [];
        $("#check-eyes").disabled = false;
        toast(error.message);
      }
    };
    eyeWorker.onerror = () => {
      eyeQueue = [];
      $("#check-eyes").disabled = false;
      toast("Не удалось запустить локальную проверку лиц");
    };
  }
  $("#check-eyes").disabled = true;
  eyeWorker.postMessage({ id: eyeQueue[0] });
}
$("#navigation").onclick = (event) => {
  const button = event.target.closest("[data-view]");
  if (button) setView(button.dataset.view);
};
for (const selector of ["#import", "#empty-import"])
  $(selector).onclick = () => $("#import-dialog").showModal();
for (const selector of ["#help", "#help-top"])
  $(selector).onclick = () => $("#help-dialog").showModal();
document.querySelectorAll(".close-dialog").forEach(
  (button) =>
    (button.onclick = () => {
      const dialog = button.closest("dialog");
      dialog.close();
      if (dialog.id === "viewer") state.culling = false;
    }),
);
$("#browse").onclick = () => pick("#folder-path");
$("#browse-export").onclick = () => pick("#export-path");
$("#import-form").onsubmit = (event) => {
  event.preventDefault();
  action(async () => {
    await api("/api/scan", { path: $("#folder-path").value.trim() });
    $("#import-dialog").close();
    state.running = true;
    await load();
  });
};
$("#stop-scan").onclick = () =>
  action(async () => {
    await api("/api/pause", {});
    toast("Пауза после текущей группы файлов…");
  });
$("#resume-scan").onclick = () =>
  action(async () => {
    await api("/api/resume", {});
    state.running = true;
    await load();
  });
$("#search").oninput = () => {
  clearTimeout(searchTimer);
  searchTimer = setTimeout(() => {
    state.offset = 0;
    state.selected.clear();
    load();
    persist();
  }, 220);
};
$("#sort").onchange = () => {
  state.offset = 0;
  state.selected.clear();
  load();
  persist();
};
$("#density").onclick = () => {
  state.settings.density =
    state.settings.density === "large" ? "normal" : "large";
  applyTheme();
  persist();
};
$("#previous").onclick = () => {
  state.offset = Math.max(0, state.offset - 80);
  state.selected.clear();
  load();
  persist();
};
$("#next").onclick = () => {
  state.offset += 80;
  state.selected.clear();
  load();
  persist();
};
$("#clear-selection").onclick = () => {
  state.selected.clear();
  updateSelection();
};
$("#select-page").onclick = () => {
  state.displayPhotos.forEach((p) => state.selected.add(p.id));
  updateSelection();
};
$("#favorite-selected").onclick = () =>
  action(async () => {
    await api("/api/favorite", {
      ids: [...state.selected],
      value: state.view !== "favorites",
    });
    state.selected.clear();
    await load();
    await status();
  });
$("#keep-selected").onclick = () => decide("keep");
$("#reject-selected").onclick = () => decide("reject");
$("#compare-selected").onclick = () => {
  state.culling = false;
  state.viewerPhotos = state.displayPhotos;
  showPhotos(state.displayPhotos.filter((p) => state.selected.has(p.id)));
};
$("#trash-selected").onclick = () => {
  if (state.view === "trash") return doMove(true);
  $("#confirm-text").textContent =
    `Выбрано: ${state.selected.size}. Связанные RAW+JPEG будут перемещены вместе. Всё можно восстановить из корзины или отменить через Ctrl+Z.`;
  $("#confirm-dialog").showModal();
};
$("#confirm-trash").onclick = () => doMove(false);
$("#cancel-trash").onclick = () => $("#confirm-dialog").close();
$("#viewer-prev").onclick = () => {
  if (state.viewerIndex > 0)
    showPhotos(
      [state.viewerPhotos[state.viewerIndex - 1]],
      state.viewerIndex - 1,
    );
};
$("#viewer-next").onclick = () => {
  if (state.viewerIndex < state.viewerPhotos.length - 1)
    showPhotos(
      [state.viewerPhotos[state.viewerIndex + 1]],
      state.viewerIndex + 1,
    );
};
$("#viewer-favorite").onclick = () =>
  action(async () => {
    const p = state.viewerPhotos[state.viewerIndex];
    await api("/api/favorite", { ids: [p.id], value: !p.favorite });
    p.favorite = !p.favorite;
    showPhotos([p], state.viewerIndex);
    await load();
    await status();
  });
$("#zoom").oninput = () => {
  pixel = false;
  zoom = Number($("#zoom").value) / 100;
  applyZoom();
};
$("#zoom-fit").onclick = resetZoom;
$("#zoom-pixel").onclick = () => {
  pixel = true;
  pan = { x: 0, y: 0 };
  applyZoom();
};
$("#fullscreen").onclick = () => {
  const viewer = $("#viewer");
  const expanded = viewer.classList.toggle("fullscreen");
  if (expanded) viewer.requestFullscreen?.().catch(() => {});
  else if (document.fullscreenElement) document.exitFullscreen();
  requestAnimationFrame(() => applyZoom());
};
window.addEventListener("resize", () => applyZoom());
$("#start-cull").onclick = () =>
  action(async () => {
    state.cullKind = ["similar", "bursts", "duplicates"].includes(state.view)
      ? state.view
      : state.settings.ui_state.cullKind || "similar";
    await nextGroup();
  });
$("#choose-winner").onclick = () => choose();
$("#skip-group").onclick = () => choose(true);
$("#undo").onclick = undo;
$("#check-eyes").onclick = analyzeEyes;
$("#settings").onclick = () => {
  $("#theme").value = state.settings.theme;
  $("#sensitivity").value = state.settings.sensitivity;
  $("#sensitivity-label").textContent = state.settings.sensitivity;
  $("#cache-limit").value = state.settings.cache_mb;
  $("#preferred-folder").value = state.settings.preferred_folder;
  $("#settings-dialog").showModal();
};
$("#sensitivity").oninput = () =>
  ($("#sensitivity-label").textContent = $("#sensitivity").value);
$("#settings-form").onsubmit = (event) => {
  event.preventDefault();
  action(async () => {
    const settings = {
      ...state.settings,
      theme: $("#theme").value,
      sensitivity: Number($("#sensitivity").value),
      cache_mb: Number($("#cache-limit").value),
      preferred_folder: $("#preferred-folder").value.trim(),
    };
    await api("/api/settings", settings);
    state.settings = settings;
    applyTheme();
    $("#settings-dialog").close();
    await load();
    toast("Настройки сохранены");
  });
};
$("#export").onclick = () => {
  $("#export-scope").textContent = state.selected.size
    ? `Будут скопированы выбранные кадры (${state.selected.size}) и их RAW+JPEG пары.`
    : "Будут скопированы все избранные кадры и их RAW+JPEG пары.";
  $("#export-dialog").showModal();
};
$("#export-form").onsubmit = (event) => {
  event.preventDefault();
  action(async () => {
    const result = await api("/api/export", {
      ids: [...state.selected],
      path: $("#export-path").value.trim(),
      structure: $("#export-structure").checked,
    });
    $("#export-dialog").close();
    outcome(result, "Экспортировано");
  });
};
$("#duplicates-plan").onclick = () =>
  action(async () => {
    const plan = await api("/api/duplicates/plan", {
      preferred_folder: state.settings.preferred_folder,
    });
    state.duplicatePlan = plan;
    $("#duplicates-summary").textContent =
      `Можно убрать ${plan.remove.length} копий (${size(plan.bytes)}). Сохраняется: ${plan.keep.length}. Защищённых избранных копий: ${plan.protected_favorites}.`;
    $("#duplicates-list").replaceChildren(
      ...plan.keep
        .slice(0, 100)
        .map((p) => element("p", "plan-keep", `✓ Сохранить: ${p.path}`)),
      ...plan.remove
        .slice(0, 100)
        .map((p) => element("p", "plan-remove", `→ В корзину: ${p.path}`)),
    );
    $("#apply-duplicates").disabled = !plan.remove.length;
    $("#duplicates-dialog").showModal();
  });
$("#apply-duplicates").onclick = () =>
  action(async () => {
    const result = await api("/api/duplicates/apply", {
      ids: state.duplicatePlan.remove.map((p) => p.id),
    });
    $("#duplicates-dialog").close();
    await load();
    await status();
    outcome(result, "В корзине");
  });
document.addEventListener("keydown", (event) => {
  if (["INPUT", "TEXTAREA", "SELECT"].includes(document.activeElement.tagName))
    return;
  if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "z") {
    event.preventDefault();
    undo();
    return;
  }
  if ($("#viewer").open) {
    if (event.key === "ArrowLeft" && !$("#viewer-prev").disabled)
      $("#viewer-prev").click();
    if (event.key === "ArrowRight" && !$("#viewer-next").disabled)
      $("#viewer-next").click();
    if (event.key.toLowerCase() === "f" && !state.comparing && !state.culling)
      $("#viewer-favorite").click();
    if (event.key.toLowerCase() === "p") {
      if (state.culling) choose();
      else decide("keep");
    }
    if (event.key.toLowerCase() === "x" && !state.comparing) decide("reject");
    if (event.key.toLowerCase() === "s") {
      if (state.culling) choose(true);
      else decide("skip");
    }
    if (event.key === "Escape") state.culling = false;
    return;
  }
  if (document.querySelector("dialog[open]")) return;
  if (event.key === "/") {
    event.preventDefault();
    $("#search").focus();
  }
  if (event.key === "?") $("#help-dialog").showModal();
  if (event.key === "Escape") {
    state.selected.clear();
    updateSelection();
  }
});
matchMedia("(prefers-color-scheme:dark)").addEventListener(
  "change",
  applyTheme,
);
async function poll() {
  try {
    const p = await api("/api/progress");
    const wasRunning = state.running;
    progress(p);
    if (p.running || wasRunning) {
      await load();
      if (wasRunning && !p.running) {
        toast(
          `${p.message}. Фото: ${p.done}. Ошибок: ${p.errors}${p.last_error ? ` — ${p.last_error}` : ""}`,
        );
        await status();
      }
    }
  } catch (error) {
    if (state.running) toast("Связь с приложением потеряна");
  } finally {
    setTimeout(poll, 1500);
  }
}
async function init() {
  try {
    state.token = (await api("/api/session")).token;
    await status();
    const saved = state.settings.ui_state || {};
    $("#sort").value = saved.sort || "date";
    $("#search").value = saved.search || "";
    setView(titles[saved.view] ? saved.view : "all", false);
    state.offset = Number(saved.offset) || 0;
    await load();
    poll();
  } catch (error) {
    toast(`Не удалось подключиться: ${error.message}`);
  }
}
init();
