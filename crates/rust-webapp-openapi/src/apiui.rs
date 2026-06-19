//! API discovery UI â€” interactive Swagger-like explorer page.
//!
//! A single-page HTML/CSS/JS application served at `/api/openapi.html`.
//! Fetches `/api/openapi.json` and renders endpoints grouped by tag
//! with expandable detail cards and a "Try it out" request builder.

/// The embedded HTML for the API docs UI.
pub const APIUI_HTML: &str = r##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>LRWF API Docs</title>
<style>
*,*::before,*::after{box-sizing:border-box;margin:0;padding:0}
body{font-family:system-ui,-apple-system,sans-serif;background:#0d1117;color:#c9d1d9;line-height:1.6}
.container{max-width:960px;margin:0 auto;padding:2rem 1rem}
.header{margin-bottom:2rem;padding-bottom:1.5rem;border-bottom:1px solid #21262d}
.header h1{font-size:1.75rem;font-weight:600;color:#f0f6fc;margin-bottom:.25rem}
.header .sub{font-size:.875rem;color:#8b949e}
.tag-group{margin-bottom:2rem}
.tag-header{display:flex;align-items:center;gap:.5rem;cursor:pointer;padding:.5rem 0;margin-bottom:.75rem;user-select:none}
.tag-header:hover .tag-title{color:#79c0ff}
.tag-arrow{color:#8b949e;font-size:.75rem;transition:transform .2s;width:16px;text-align:center}
.tag-arrow.open{transform:rotate(90deg)}
.tag-title{font-size:1.05rem;font-weight:600;color:#58a6ff;text-transform:capitalize}
.tag-count{font-size:.75rem;color:#8b949e;background:#21262d;padding:1px 8px;border-radius:10px}
.tag-body{display:none}
.tag-body.open{display:block}
.endpoint{margin-bottom:.5rem;border:1px solid #21262d;border-radius:8px;background:#161b22;overflow:hidden;transition:border-color .15s}
.endpoint:hover{border-color:#30363d}
.ep-summary{display:flex;align-items:center;padding:.625rem 1rem;cursor:pointer;gap:.75rem;user-select:none}
.ep-summary:hover{background:#1c2129}
.method{display:inline-block;min-width:60px;padding:2px 8px;border-radius:4px;font-size:.75rem;font-weight:700;text-align:center;text-transform:uppercase;flex-shrink:0}
.method.get{background:#1b3a2d;color:#3fb950}
.method.post{background:#1b3a4a;color:#58a6ff}
.method.put{background:#3a2d1b;color:#d29922}
.method.delete{background:#3a1b1b;color:#f85149}
.method.patch{background:#2d1b3a;color:#bc8cff}
.ep-path{font-family:'SFMono-Regular',Consolas,monospace;font-size:.9rem;color:#e6edf3;flex:1}
.ep-summary-text{font-size:.8rem;color:#8b949e;margin-left:auto;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;max-width:200px}
.ep-detail{display:none;border-top:1px solid #21262d;padding:1rem}
.ep-detail.open{display:block}
.ep-section{margin-bottom:1.25rem}
.ep-section:last-child{margin-bottom:0}
.ep-section-title{font-size:.75rem;font-weight:600;color:#8b949e;text-transform:uppercase;margin-bottom:.5rem;letter-spacing:.5px}
.param-table{width:100%;border-collapse:collapse;font-size:.85rem}
.param-table th{text-align:left;padding:.35rem .5rem;color:#8b949e;font-weight:600;font-size:.75rem;text-transform:uppercase;border-bottom:1px solid #21262d}
.param-table td{padding:.35rem .5rem;border-bottom:1px solid #1a1f27}
.param-table tr:last-child td{border-bottom:none}
.param-name{font-family:'SFMono-Regular',Consolas,monospace;color:#e6edf3}
.param-in{color:#d2a8ff;font-size:.8rem}
.param-type{font-family:'SFMono-Regular',Consolas,monospace;color:#7ee787;font-size:.82rem}
.param-required{color:#f85149;font-weight:700;font-size:.75rem}
.body-example{background:#0d1117;border:1px solid #21262d;border-radius:6px;padding:.75rem;overflow-x:auto}
.body-example pre{font-family:'SFMono-Regular',Consolas,monospace;font-size:.8rem;color:#e6edf3;white-space:pre-wrap;word-break:break-all}
.ep-description{font-size:.85rem;color:#8b949e;margin-bottom:.75rem;line-height:1.6;padding:.5rem .75rem;background:#0d1117;border-radius:6px;border:1px solid #21262d}
.resp-list{list-style:none}
.resp-item{display:flex;align-items:center;gap:.5rem;padding:.35rem 0;font-size:.85rem}
.resp-status{display:inline-block;min-width:36px;padding:1px 6px;border-radius:4px;font-size:.75rem;font-weight:600;text-align:center}
.resp-status.s2xx{background:#1b3a2d;color:#3fb950}
.resp-status.s4xx{background:#3a2d1b;color:#d29922}
.resp-status.s5xx{background:#3a1b1b;color:#f85149}
.resp-desc{color:#c9d1d9}
.copy-btn{display:inline-block;padding:2px 10px;font-size:.75rem;color:#58a6ff;background:#1b3a4a;border:1px solid #30363d;border-radius:4px;cursor:pointer;margin-top:.5rem;transition:background .15s}
.copy-btn:hover{background:#1f4050}
.copy-btn.copied{background:#1b3a2d;color:#3fb950;border-color:#3fb950}
.try-btn{display:inline-block;padding:2px 10px;font-size:.75rem;color:#d29922;background:#3a2d1b;border:1px solid #5a4622;border-radius:4px;cursor:pointer;margin-top:.5rem;margin-left:.5rem;transition:background .15s}
.try-btn:hover{background:#4a3d2b}
.try-btn.active{background:#d29922;color:#0d1117;border-color:#d29922}
.try-panel{display:none;margin-top:.75rem;padding:.75rem;background:#0a0e14;border:1px solid #21262d;border-radius:6px}
.try-panel.open{display:block}
.try-panel .path-builder{font-family:'SFMono-Regular',Consolas,monospace;font-size:.85rem;color:#e6edf3;margin-bottom:.75rem;padding:.5rem .75rem;background:#12171f;border-radius:4px;display:flex;flex-wrap:wrap;align-items:center;gap:2px}
.try-panel .path-builder span{color:#8b949e}
.try-panel .path-builder input{font-family:'SFMono-Regular',Consolas,monospace;font-size:.8rem;background:#0d1117;color:#e6edf3;border:1px solid #30363d;border-radius:3px;padding:2px 6px;min-width:80px;outline:none;transition:border-color .15s}
.try-panel .path-builder input:focus{border-color:#58a6ff}
.try-panel .body-input{margin-bottom:.75rem}
.try-panel .body-input textarea{width:100%;min-width:100%;max-width:100%;min-height:80px;height:120px;resize:vertical;font-family:'SFMono-Regular',Consolas,monospace;font-size:.8rem;background:#0d1117;color:#e6edf3;border:1px solid #30363d;border-radius:4px;padding:.5rem .75rem;outline:none;transition:border-color .15s;line-height:1.5;tab-size:2}
.try-panel .body-input textarea:focus{border-color:#58a6ff}
.try-panel .body-input label{display:block;font-size:.7rem;font-weight:600;color:#8b949e;text-transform:uppercase;margin-bottom:.35rem;letter-spacing:.5px}
.try-panel .send-btn{display:inline-block;padding:4px 16px;font-size:.75rem;font-weight:600;color:#fff;background:#238636;border:1px solid #2ea043;border-radius:4px;cursor:pointer;transition:background .15s}
.try-panel .send-btn:hover{background:#2ea043}
.try-panel .send-btn:disabled{opacity:.5;cursor:not-allowed}
.resp-area{display:none;margin-top:.75rem;padding:.75rem;background:#0a0e14;border:1px solid #21262d;border-radius:6px}
.resp-area.open{display:block}
.resp-area .status{font-size:1.5rem;font-weight:700;margin-bottom:.5rem;padding-bottom:.5rem;border-bottom:1px solid #21262d}
.resp-area .status.s2xx{color:#3fb950}
.resp-area .status.s4xx{color:#d29922}
.resp-area .status.s5xx{color:#f85149}
.resp-area .body{font-family:'SFMono-Regular',Consolas,monospace;font-size:.8rem;color:#e6edf3;white-space:pre-wrap;word-break:break-all;background:#0d1117;padding:.5rem .75rem;border-radius:4px;max-height:400px;overflow-y:auto;line-height:1.5}
.resp-area .loading-text{color:#8b949e;font-size:.85rem}
.resp-area .error-text{color:#f85149;font-size:.85rem}
.loading{text-align:center;padding:3rem 0;color:#8b949e}
.error{text-align:center;padding:2rem;background:#3a1b1b;border-radius:6px;color:#f85149}
.empty{text-align:center;padding:3rem 0;color:#8b949e}
.badge{display:inline-block;padding:1px 6px;border-radius:4px;font-size:.7rem;font-weight:600;margin-left:.35rem}
.badge.req{background:#3a2d1b;color:#d29922}
</style>
</head>
<body>
<div class="container">
  <div class="header">
    <h1 id="title">API Documentation</h1>
    <div class="sub" id="version"></div>
  </div>
  <div id="content"><div class="loading">Loading API specification...</div></div>
</div>
<script>
(function(){
var content=document.getElementById('content');

function h(s){return String(s).replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;').replace(/"/g,'&quot;');}

function buildBodyExample(params){
  var obj={};
  for(var i=0;i<params.length;i++){
    var p=params[i];
    if(p.source==='body'){obj.id='string';obj.name='string';obj.email='string';break;}
  }
  return JSON.stringify(obj,null,2);
}

function buildCurl(method,path,params){
  var curl='curl -X '+method;
  if(params){
    var hasBody=false;
    for(var i=0;i<params.length;i++){
      if(params[i].source==='body'){hasBody=true;break;}
    }
    if(hasBody){
      curl+=' -H "Content-Type: application/json" -d '+JSON.stringify(JSON.stringify({id:'string',name:'string',email:'string'}));
    }
  }
  curl+=' "http://localhost:5000'+path+'"';
  return curl;
}

function ucFirst(s){return s.charAt(0).toUpperCase()+s.slice(1);}

function safeParams(params){return params||[];}

function statusClass(code){
  if(code>=200&&code<300)return's2xx';
  if(code>=400&&code<500)return's4xx';
  return's5xx';
}

function hasBodyMethod(method){
  return method==='POST'||method==='PUT'||method==='PATCH';
}

function extractPathParams(path){
  var re=/\{(\w+)\}/g;
  var params=[];
  var match;
  while((match=re.exec(path))!==null){
    params.push(match[1]);
  }
  return params;
}

function buildPathBuilderHTML(path){
  var parts=[];
  var last=0;
  var re=/\{(\w+)\}/g;
  var match;
  while((match=re.exec(path))!==null){
    if(match.index>last){
      parts.push('<span>'+h(path.slice(last,match.index))+'</span>');
    }
    parts.push('<input type="text" class="path-param" data-param="'+h(match[1])+'" placeholder="'+h(match[1])+'" value="">');
    last=re.lastIndex;
  }
  if(last<path.length){
    parts.push('<span>'+h(path.slice(last))+'</span>');
  }
  if(parts.length===0){
    parts.push('<span>'+h(path)+'</span>');
  }
  return parts.join('');
}

function buildTryPanelHTML(epId,method,path,params,requestBody){
  var pathParams=extractPathParams(path);
  var hasBody=hasBodyMethod(method);
  var html='';
  html+='<div class="try-panel" id="try-'+epId+'">';

  if(pathParams.length>0){
    html+='<div class="path-builder">'+buildPathBuilderHTML(path)+'</div>';
  }

  if(hasBody){
    var bodyExample=requestBody||buildBodyExample(params);
    html+='<div class="body-input">';
    html+='<label>Request Body (JSON)</label>';
    html+='<textarea class="try-body" id="req-body-'+epId+'">'+h(bodyExample)+'</textarea>';
    html+='</div>';
  }

  html+='<button class="send-btn" onclick="executeRequest(\''+epId+'\',\''+h(method)+'\',\''+h(path)+'\')">Send</button>';
  html+='<div class="resp-area" id="resp-'+epId+'">';
  html+='<div class="status" id="status-'+epId+'"></div>';
  html+='<div class="body" id="resp-body-'+epId+'"></div>';
  html+='</div>';
  html+='</div>';
  return html;
}

function buildDescriptionHTML(description){
  if(!description)return'';
  return'<div class="ep-description">'+h(description).replace(/\n/g,'<br>')+'</div>';
}

fetch('/api/openapi.json')
.then(function(r){if(!r.ok)throw new Error('HTTP '+r.status);return r.json();})
.then(function(spec){
  document.getElementById('title').textContent=spec.info.title||'API Documentation';
  document.getElementById('version').textContent='v'+(spec.info.version||'0.0.0');

  var paths=spec.paths||{};
  var entries=Object.entries(paths);
  if(entries.length===0){content.innerHTML='<div class="empty">No endpoints registered.</div>';return;}

  var groups=new Map();
  for(var i=0;i<entries.length;i++){
    var path=entries[i][0];
    var methods=entries[i][1];
    var methodKeys=Object.keys(methods);
    for(var j=0;j<methodKeys.length;j++){
      var method=methodKeys[j];
      var op=methods[method];
      var tags=op.tags||['default'];
      for(var k=0;k<tags.length;k++){
        var tag=tags[k];
        if(!groups.has(tag))groups.set(tag,[]);
        groups.get(tag).push({
          method:method.toUpperCase(),
          path:path,
          summary:op.summary||'',
          description:op.description||'',
          operationId:op.operationId||'',
          parameters:safeParams(op.parameters),
          requestBody:op.requestBody||null,
          responses:op.responses||{},
        });
      }
    }
  }

  var html='';
  var tagKeys=Array.from(groups.keys());
  for(var ti=0;ti<tagKeys.length;ti++){
    var tag=tagKeys[ti];
    var eps=groups.get(tag);
    html+='<div class="tag-group">';
    html+='<div class="tag-header" onclick="var b=this.nextElementSibling;var a=this.querySelector(\'.tag-arrow\');b.classList.toggle(\'open\');a.classList.toggle(\'open\')">';
    html+='<span class="tag-arrow open">&#9654;</span>';
    html+='<span class="tag-title">'+h(tag)+'</span>';
    html+='<span class="tag-count">'+eps.length+'</span>';
    html+='</div>';
    html+='<div class="tag-body open">';

    for(var ei=0;ei<eps.length;ei++){
      var ep=eps[ei];
      var epId='ep-'+ti+'-'+ei;
      html+='<div class="endpoint">';
      html+='<div class="ep-summary" onclick="var d=document.getElementById(\''+epId+'\');d.classList.toggle(\'open\')">';
      html+='<span class="method '+ep.method.toLowerCase()+'">'+ep.method+'</span>';
      html+='<span class="ep-path">'+h(ep.path)+'</span>';
      html+='<span class="ep-summary-text">'+h(ep.summary)+'</span>';
      html+='</div>';
      html+='<div class="ep-detail" id="'+epId+'">';

      html+=buildDescriptionHTML(ep.description);

      if(ep.parameters.length>0){
        html+='<div class="ep-section"><div class="ep-section-title">Parameters</div>';
        html+='<table class="param-table"><tr><th>Name</th><th>In</th><th>Type</th><th>Required</th></tr>';
        for(var pi=0;pi<ep.parameters.length;pi++){
          var p=ep.parameters[pi];
          var ptype=p.schema?p.schema.type:'string';
          html+='<tr><td class="param-name">'+h(p.name)+'</td>';
          html+='<td class="param-in">'+h(p.in)+'</td>';
          html+='<td class="param-type">'+h(ptype)+'</td>';
          html+='<td class="param-required">'+(p.required?'required':'')+'</td></tr>';
        }
        html+='</table></div>';
      }

      if(ep.requestBody){
        html+='<div class="ep-section"><div class="ep-section-title">Request Body <span class="badge req">application/json</span></div>';
        html+='<div class="body-example"><pre>'+h(buildBodyExample(ep.parameters))+'</pre></div></div>';
      }

      var respKeys=Object.keys(ep.responses).sort();
      if(respKeys.length>0){
        html+='<div class="ep-section"><div class="ep-section-title">Responses</div><ul class="resp-list">';
        for(var ri=0;ri<respKeys.length;ri++){
          var code=respKeys[ri];
          var resp=ep.responses[code];
          html+='<li class="resp-item"><span class="resp-status '+statusClass(parseInt(code))+'">'+h(code)+'</span><span class="resp-desc">'+h(resp.description||'')+'</span></li>';
        }
        html+='</ul></div>';
      }

      html+='<button class="copy-btn" data-curl="'+h(buildCurl(ep.method,ep.path,ep.parameters))+'">Copy curl</button>';
      html+='<button class="try-btn" data-ep="'+epId+'">Try it</button>';
      html+=buildTryPanelHTML(epId,ep.method,ep.path,ep.parameters,ep.requestBody);
      html+='</div></div>';
    }

    html+='</div></div>';
  }

  content.innerHTML=html;

  // --- Event Delegation ---
  content.addEventListener('click',function(e){
    // Copy-curl button
    var btn=e.target.closest('.copy-btn');
    if(btn){
      var curl=btn.getAttribute('data-curl');
      if(!curl)return;
      navigator.clipboard.writeText(curl).then(function(){
        btn.textContent='Copied!';
        btn.classList.add('copied');
        setTimeout(function(){
          btn.textContent='Copy curl';
          btn.classList.remove('copied');
        },2000);
      }).catch(function(){
        btn.textContent='Error';
      });
      return;
    }

    // Try-it button
    var tryBtn=e.target.closest('.try-btn');
    if(tryBtn){
      var epId=tryBtn.getAttribute('data-ep');
      var panel=document.getElementById('try-'+epId);
      if(!panel)return;
      var isOpen=panel.classList.contains('open');
      if(isOpen){
        panel.classList.remove('open');
        tryBtn.classList.remove('active');
        tryBtn.textContent='Try it';
      }else{
        panel.classList.add('open');
        tryBtn.classList.add('active');
        tryBtn.textContent='Hide';
      }
      return;
    }
  });
})
.catch(function(e){
  content.innerHTML='<div class="error">Failed to load API spec: '+h(e.message)+'</div>';
});
})();

// Global request execution function
function executeRequest(epId,method,path){
  var panel=document.getElementById('try-'+epId);
  var respArea=document.getElementById('resp-'+epId);
  var statusEl=document.getElementById('status-'+epId);
  var bodyEl=document.getElementById('resp-body-'+epId);
  var sendBtn=panel?panel.querySelector('.send-btn'):null;

  respArea.classList.add('open');
  statusEl.textContent='';
  statusEl.className='status';
  bodyEl.textContent='Sending...';
  bodyEl.className='body';

  if(sendBtn)sendBtn.disabled=true;

  // Build URL by replacing path parameters
  var url=path;
  var pathInputs=panel?panel.querySelectorAll('.path-param'):[];
  for(var i=0;i<pathInputs.length;i++){
    var input=pathInputs[i];
    var paramName=input.getAttribute('data-param');
    var value=input.value.trim();
    if(!value)value=paramName;
    url=url.replace('{'+paramName+'}',encodeURIComponent(value));
  }

  var fetchOpts={method:method,headers:{}};

  // Check for body
  if(method==='POST'||method==='PUT'||method==='PATCH'){
    var textarea=document.getElementById('req-body-'+epId);
    if(textarea&&textarea.value.trim()){
      fetchOpts.headers['Content-Type']='application/json';
      fetchOpts.body=textarea.value;
    }
  }

  fetch(url,fetchOpts)
  .then(function(resp){
    var sc=resp.status>=200&&resp.status<300?'s2xx':resp.status>=400&&resp.status<500?'s4xx':'s5xx';
    statusEl.textContent=resp.status+' '+resp.statusText;
    statusEl.className='status '+sc;
    return resp.text().then(function(txt){return{status:resp.status,body:txt};});
  })
  .then(function(result){
    var display=result.body;
    try{
      var parsed=JSON.parse(result.body);
      display=JSON.stringify(parsed,null,2);
    }catch(e){}
    bodyEl.textContent=display;
    bodyEl.className='body';
    if(sendBtn)sendBtn.disabled=false;
  })
  .catch(function(err){
    statusEl.textContent='Error';
    statusEl.className='status s5xx';
    bodyEl.textContent=err.message||'Request failed';
    bodyEl.className='body error-text';
    if(sendBtn)sendBtn.disabled=false;
  });
}
</script>
</body>
</html>
"##;
