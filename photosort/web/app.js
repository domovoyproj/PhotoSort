const $ = selector => document.querySelector(selector);
const titles = {all:'Все фотографии',favorites:'Избранное',similar:'Похожие кадры',bursts:'Серии',duplicates:'Точные дубли',trash:'Корзина'};
const state = {view:'all',offset:0,photos:[],selected:new Set(),token:'',running:false,viewerIndex:0,request:0};
const number = value => Number(value).toLocaleString('ru-RU');
const size = value => value > 1e9 ? `${(value/1e9).toFixed(1)} ГБ` : `${(value/1e6).toFixed(1)} МБ`;
let toastTimer, searchTimer;

function toast(message) {
  $('#toast').textContent = message; $('#toast').hidden = false;
  clearTimeout(toastTimer); toastTimer = setTimeout(() => $('#toast').hidden = true,6000);
}
async function api(path, body) {
  const response = await fetch(path,body === undefined ? {} : {method:'POST',headers:{'Content-Type':'application/json','X-PhotoSort-Token':state.token},body:JSON.stringify(body)});
  const data = await response.json();
  if (!response.ok) throw new Error(data.error || 'Ошибка запроса');
  return data;
}
function element(tag,className,text) {
  const node = document.createElement(tag); if(className) node.className=className;
  if(text !== undefined) node.textContent=text; return node;
}
function updateSelection() {
  $('#selection-bar').hidden = state.selected.size === 0;
  $('#selection-count').textContent = `Выбрано: ${state.selected.size}`;
  $('#trash-selected').textContent = state.view === 'trash' ? 'Восстановить' : 'В корзину';
  $('#compare-selected').disabled = state.selected.size !== 2;
  $('#favorite-selected').textContent = state.view === 'favorites' ? '♡ Убрать из избранного' : '♡ В избранное';
  document.querySelectorAll('.photo-card').forEach(card => {
    const selected = state.selected.has(Number(card.dataset.id));
    card.classList.toggle('selected',selected); card.querySelector('input').checked=selected;
  });
}
function card(photo,index) {
  const item=element('article','photo-card'); item.dataset.id=photo.id;
  const frame=element('div','photo-image'), open=element('button','photo-open');
  open.title=`Открыть ${photo.name}`; open.setAttribute('aria-label',open.title);
  const image=element('img'); image.src=`/media/${photo.id}`; image.alt=photo.name; image.loading='lazy';
  open.append(image); open.onclick=()=>{state.viewerPhotos=[...state.photos];view([photo],index)}; frame.append(open);
  const check=element('input','photo-select'); check.type='checkbox'; check.setAttribute('aria-label',`Выбрать ${photo.name}`);
  check.onchange=()=>{check.checked?state.selected.add(photo.id):state.selected.delete(photo.id);updateSelection()};frame.append(check);
  const heart=element('button','photo-heart',photo.favorite?'♥':'♡'); heart.title=photo.favorite?'Убрать из избранного':'В избранное';
  heart.setAttribute('aria-label',heart.title); heart.onclick=async()=>{try{await api('/api/favorite',{ids:[photo.id],value:!photo.favorite});await load()}catch(e){toast(e.message)}};frame.append(heart);
  const badge={duplicates:`Дубль · ${photo.hash.slice(0,6)}`,similar:`Группа ${photo.similar}`,bursts:`Серия ${photo.burst}`,trash:'В корзине'}[state.view];
  if(badge)frame.append(element('span','photo-badge',badge));
  const caption=element('div','photo-caption');caption.append(element('strong','',photo.name),element('span','',size(photo.size)));
  const date=photo.captured?new Date(photo.captured*1000).toLocaleDateString('ru-RU'):'Без даты съёмки';
  item.append(frame,caption,element('div','photo-detail',`${photo.width} × ${photo.height}  ·  ${date}`));return item;
}
async function load() {
  const request=++state.request;
  try {
    const query=new URLSearchParams({view:state.view,offset:state.offset,search:$('#search').value,sort:$('#sort').value});
    const data=await api(`/api/photos?${query}`); if(request!==state.request)return;
    if(!data.photos.length && state.offset>0){state.offset=Math.max(0,state.offset-80);return load()}
    state.photos=data.photos;
    for(const key of Object.keys(titles))$(`#count-${key}`).textContent=number(data.counts[key]);
    $('#stat-total').replaceChildren(document.createTextNode(number(data.counts.all)+' '),element('small','','фото'));
    $('#stat-similar').textContent=number(data.counts.similar);$('#stat-duplicates').textContent=number(data.counts.duplicates);
    $('#stat-favorites').replaceChildren(document.createTextNode(number(data.counts.favorites)+' '),element('span','','♡'));
    $('#stat-bytes').textContent=`${size(data.counts.bytes)} на диске`;
    $('#total-label').textContent=`${number(data.total)} фотографий`;
    $('#gallery').replaceChildren(...state.photos.map(card));
    $('#empty').hidden=data.total!==0;
    const first=state.view==='all'&&!$('#search').value&&!data.counts.all;
    $('#empty-title').textContent=first?'Всё начинается с одной папки':state.view==='trash'?'Корзина пуста':'Здесь пока нет фотографий';
    $('#empty-description').textContent=first?'Добавьте фотографии. PhotoSort найдёт похожие кадры, объединит серии и поможет оставить самое ценное.':state.view==='trash'?'Перемещённые сюда фотографии можно восстановить в исходную папку.':'Попробуйте другой поиск или добавьте фотографии в библиотеку.';
    $('#empty-import').hidden=!first;
    $('#previous').disabled=state.offset===0;$('#next').disabled=state.offset+80>=data.total;
    $('#page-number').textContent=String(Math.floor(state.offset/80)+1).padStart(2,'0');
    $('#footer-text').textContent=data.total?`${state.offset+1}–${Math.min(state.offset+80,data.total)} из ${number(data.total)} фотографий`:'Порядок начинается здесь';
    progress(data.progress);updateSelection();
  }catch(error){toast(`Не удалось загрузить библиотеку: ${error.message}`)}
}
function progress(data) {
  state.running=data.running;$('#scan-status').hidden=!data.running;
  $('#scan-text').textContent=`Обработано ${number(data.done)} фото`;
  $('#scan-detail').textContent=`${data.message}${data.errors?` · ошибок: ${data.errors}`:''}`;
  $('#stop-scan').disabled=!data.running;
}
function setView(viewName) {
  state.view=viewName;state.offset=0;state.selected.clear();
  document.querySelectorAll('[data-view]').forEach(button=>button.classList.toggle('active',button.dataset.view===viewName));
  $('#page-title').replaceChildren(document.createTextNode(titles[viewName]),element('span','title-dot','.'));
  $('#crumb').textContent=titles[viewName];$('#collection-name').textContent=viewName==='all'?'Коллекция':titles[viewName];
  const hints={similar:'Кадры сгруппированы по визуальному сходству. Сравните фотографии перед выбором: похожие кадры могут отличаться важными деталями.',bursts:'Серии: соседние кадры из одной папки с интервалом до 8 секунд по EXIF. Фотографии без времени съёмки в серии не включаются.',duplicates:'Точные дубли имеют одинаковое содержимое файла (SHA-256). Выделите лишние копии для перемещения в корзину.',trash:'Файлы хранятся в .photosort-trash внутри исходной папки. Выделите кадры и нажмите «Восстановить». Автоматической очистки нет.'};
  $('#view-hint').hidden=!hints[viewName];$('#view-hint').textContent=hints[viewName]||'';load();
}
function view(photos,index=0) {
  state.viewerIndex=index;state.comparing=photos.length===2;
  $('#viewer-title').textContent=state.comparing?'Сравнение кадров':photos[0].name;
  $('#viewer-images').replaceChildren(...photos.map(photo=>{
    const figure=element('figure'),image=element('img');image.src=`/media/${photo.id}?full=1`;image.alt=photo.name;
    figure.append(image,element('figcaption','',`${photo.name} · ${photo.width} × ${photo.height} · резкость ${photo.sharpness}`));return figure;
  }));
  $('#viewer-favorite').hidden=state.comparing;$('#viewer-favorite').textContent=photos[0].favorite?'♥ В избранном':'♡ В избранное';
  $('#viewer-prev').disabled=state.comparing||index===0;$('#viewer-next').disabled=state.comparing||index>=(state.viewerPhotos||[]).length-1;
  if(!$('#viewer').open)$('#viewer').showModal();
}
async function moveSelected() {
  const restore=state.view==='trash';
  if(!restore){$('#confirm-text').textContent=`Фотографий: ${state.selected.size}. Их можно будет вернуть в исходную папку из раздела «Корзина».`;$('#confirm-dialog').showModal();return}
  await doMove(true);
}
async function doMove(restore) {
  $('#confirm-dialog').close();
  try{const data=await api(restore?'/api/restore':'/api/trash',{ids:[...state.selected]});state.selected.clear();await load();toast(`${restore?'Восстановлено':'В корзине'}: ${data.done.length}${data.errors.length?`. Ошибок: ${data.errors.length}. ${data.errors[0].message}`:''}`)}catch(e){toast(e.message)}
}
$('#navigation').onclick=event=>{const button=event.target.closest('[data-view]');if(button)setView(button.dataset.view)};
for(const selector of ['#import','#empty-import'])$(selector).onclick=()=>$('#import-dialog').showModal();
for(const selector of ['#help','#help-top'])$(selector).onclick=()=>$('#help-dialog').showModal();
document.querySelectorAll('.close-dialog').forEach(button=>button.onclick=()=>button.closest('dialog').close());
$('#browse').onclick=async()=>{const button=$('#browse');button.disabled=true;try{const data=await api('/api/pick',{});if(data.path)$('#folder-path').value=data.path}catch(e){toast(e.message)}finally{button.disabled=false}};
$('#import-form').onsubmit=async event=>{event.preventDefault();try{await api('/api/scan',{path:$('#folder-path').value.trim()});$('#import-dialog').close();state.running=true;await load()}catch(e){toast(e.message)}};
$('#stop-scan').onclick=async()=>{try{await api('/api/stop',{});toast('Остановка после текущего файла…')}catch(e){toast(e.message)}};
$('#search').oninput=()=>{clearTimeout(searchTimer);searchTimer=setTimeout(()=>{state.offset=0;state.selected.clear();load()},220)};
$('#sort').onchange=()=>{state.offset=0;state.selected.clear();load()};
$('#density').onclick=()=>$('#gallery').classList.toggle('large');
$('#previous').onclick=()=>{state.offset=Math.max(0,state.offset-80);state.selected.clear();load()};
$('#next').onclick=()=>{state.offset+=80;state.selected.clear();load()};
$('#clear-selection').onclick=()=>{state.selected.clear();updateSelection()};
$('#select-page').onclick=()=>{state.photos.forEach(photo=>state.selected.add(photo.id));updateSelection()};
$('#favorite-selected').onclick=async()=>{try{await api('/api/favorite',{ids:[...state.selected],value:state.view!=='favorites'});state.selected.clear();load()}catch(e){toast(e.message)}};
$('#compare-selected').onclick=()=>view(state.photos.filter(photo=>state.selected.has(photo.id)));
$('#trash-selected').onclick=moveSelected;$('#confirm-trash').onclick=()=>doMove(false);$('#cancel-trash').onclick=()=>$('#confirm-dialog').close();
$('#viewer-prev').onclick=()=>{if(state.viewerIndex>0)view([state.viewerPhotos[state.viewerIndex-1]],state.viewerIndex-1)};
$('#viewer-next').onclick=()=>{if(state.viewerIndex<state.viewerPhotos.length-1)view([state.viewerPhotos[state.viewerIndex+1]],state.viewerIndex+1)};
$('#viewer-favorite').onclick=async()=>{const photo=state.viewerPhotos[state.viewerIndex];try{await api('/api/favorite',{ids:[photo.id],value:!photo.favorite});photo.favorite=!photo.favorite;await load();view([photo],state.viewerIndex)}catch(e){toast(e.message)}};
document.addEventListener('keydown',event=>{
  if(['INPUT','TEXTAREA','SELECT'].includes(document.activeElement.tagName))return;
  if($('#viewer').open){if(event.key==='ArrowLeft'&&!$('#viewer-prev').disabled)$('#viewer-prev').click();if(event.key==='ArrowRight'&&!$('#viewer-next').disabled)$('#viewer-next').click();if(event.key.toLowerCase()==='f'&&!state.comparing)$('#viewer-favorite').click();return}
  if(document.querySelector('dialog[open]'))return;
  if(event.key==='/'){event.preventDefault();$('#search').focus()}
  if(event.key==='?')$('#help-dialog').showModal();
  if(event.key==='Escape'){state.selected.clear();updateSelection()}
});
async function poll(){try{const data=await api('/api/progress');const wasRunning=state.running;progress(data);if(data.running||wasRunning){await load();if(wasRunning&&!data.running)toast(`${data.message}. Фото: ${data.done}. Ошибок: ${data.errors}${data.last_error?` — ${data.last_error}`:''}`)}}catch(e){if(state.running)toast('Связь с приложением потеряна')}finally{setTimeout(poll,1500)}}
async function init(){try{state.token=(await api('/api/session')).token;await load();poll()}catch(e){toast(`Не удалось подключиться: ${e.message}`)}}
init();
