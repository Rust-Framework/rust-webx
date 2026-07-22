/* global React, ReactDOM, antd */
const {
  Layout, Typography, Button, Space, Table, Input, Select, Modal, Form,
  message, ConfigProvider, Avatar, Divider, Row, Col, Popconfirm, Tag, Empty,
  Tooltip,
} = antd;
const { Header, Content } = Layout;
const { Text, Title, Paragraph } = Typography;

/** iconfont helper: returns <i className="iconfont icon-{name}" /> */
const ic = (name) => React.createElement('i', { className: `iconfont icon-${name}` });

// Table cell styles for help modal
const c0 = { padding: '6px 8px', borderBottom: '1px solid #e8e8e8' };
const c1 = { padding: '6px 8px', borderBottom: '1px solid #e8e8e8' };
const c2 = { padding: '6px 8px', borderBottom: '1px solid #e8e8e8', background: '#fafafa' };
const c3 = { padding: '6px 8px', borderBottom: '1px solid #e8e8e8', background: '#fafafa' };

if (!window.DmbitApi.getToken()) {
  location.replace('/admin/login.html');
}

const CATEGORY_OPTIONS = [
  { value: 'compute', label: '算力' },
  { value: 'storage', label: '存储' },
];
const PARAM_KEYS = ['机箱', '主板', '内存', '接口', '扩展', '电源', '光模块'];
const PARAM_KEY_SET = new Set(PARAM_KEYS);

const zhCN = antd.locales?.zh_CN;

function categoryLabel(cat) {
  const hit = CATEGORY_OPTIONS.find((c) => c.value === cat);
  return hit ? hit.label : cat || '—';
}

function paramMap(text) {
  const map = {};
  String(text || '')
    .split(/\r?\n/)
    .map((s) => s.trim())
    .filter(Boolean)
    .forEach((line) => {
      const m = line.match(/^([^:：]+)[:：]\s*(.*)$/);
      if (m) map[m[1].trim()] = m[2].trim();
    });
  return map;
}

function extraParamRows(text) {
  const map = paramMap(text);
  return Object.keys(map)
    .filter((k) => !PARAM_KEY_SET.has(k))
    .map((k) => ({ key: k, value: map[k] || '' }));
}

function buildParameters(values, extras) {
  const lines = [];
  PARAM_KEYS.forEach((k) => {
    const v = (values && values[k] != null ? String(values[k]) : '').trim();
    if (v) lines.push(k + '：' + v);
  });
  (extras || []).forEach((row) => {
    const key = String(row.key || '').trim();
    const value = String(row.value || '').trim();
    if (!key || PARAM_KEY_SET.has(key)) return;
    if (value) lines.push(key + '：' + value);
  });
  return lines.join('\n');
}

function brandSummary(specs) {
  const brands = [...new Set((specs || []).map((s) => s.brand).filter(Boolean))];
  return brands.length ? brands.join('、') : '—';
}

function specTotal(specs) {
  return (specs || []).reduce((s, sp) => s + (Number(sp.planned_quantity) || 0), 0);
}

function formatCapacityLabel(gb) {
  const n = Number(gb) || 0;
  if (n <= 0) return '';
  if (n % 1000000 === 0) return n / 1000000 + 'PB';
  if (n % 1000 === 0) return n / 1000 + 'TB';
  return n + 'GB';
}

function formatComponents(components) {
  const list = components || [];
  if (!list.length) return '—';
  return list
    .map((c) => {
      const model = String(c.model || '').trim() || '?';
      const qty = Number(c.qty_per_unit) || 0;
      if (c.kind === 'disk' && c.capacity_gb > 0) {
        return model + '·' + formatCapacityLabel(c.capacity_gb) + '×' + qty;
      }
      return model + '×' + qty;
    })
    .join(' · ');
}

function partsSummary(spec) {
  if (spec && spec.parts_summary) return spec.parts_summary;
  return formatComponents(spec && spec.components);
}

function downloadCsv(csv, filename) {
  const BOM = '\uFEFF';
  const text = typeof csv === 'string' ? csv : String(csv || '');
  const withBom = text.startsWith(BOM) ? text : BOM + text;
  const blob = new Blob([withBom], { type: 'text/csv;charset=utf-8' });
  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = url;
  a.download = filename;
  document.body.appendChild(a);
  a.click();
  a.remove();
  URL.revokeObjectURL(url);
}

function App() {
  const user = window.DmbitApi.getUser() || {};
  const [products, setProducts] = React.useState([]);
  const [search, setSearch] = React.useState('');
  const [loading, setLoading] = React.useState(false);
  const [importing, setImporting] = React.useState(false);
  const [exporting, setExporting] = React.useState(false);
  const [expandedKeys, setExpandedKeys] = React.useState([]);
  const importRef = React.useRef(null);

  const [productOpen, setProductOpen] = React.useState(false);
  const [editingProduct, setEditingProduct] = React.useState(null);
  const [productForm] = Form.useForm();

  const [specOpen, setSpecOpen] = React.useState(false);
  const [specProductId, setSpecProductId] = React.useState(null);
  const [specProductCategory, setSpecProductCategory] = React.useState('compute');
  const [editingSpec, setEditingSpec] = React.useState(null);
  const [specForm] = Form.useForm();

  const [pwdOpen, setPwdOpen] = React.useState(false);
  const [pwdForm] = Form.useForm();
  const [helpOpen, setHelpOpen] = React.useState(false);

  const load = React.useCallback(async () => {
    setLoading(true);
    try {
      const list = await window.DmbitApi.get('/api/products');
      setProducts(list || []);
    } catch (ex) {
      if (ex.status === 401) {
        window.DmbitApi.clearSession();
        location.replace('/admin/login.html');
        return;
      }
      message.error(ex.message || '加载失败');
    } finally {
      setLoading(false);
    }
  }, []);

  React.useEffect(() => {
    load();
  }, [load]);

  const filtered = React.useMemo(() => {
    const q = search.trim().toLowerCase();
    return products.filter((p) => {
      if (!q) return true;
      const specs = p.goods || [];
      const brands = specs.map((s) => s.brand || '').join(' ');
      const codes = specs.map((s) => s.code || '').join(' ');
      const hay = [p.name, p.code, p.remark, p.category, brands, codes].join(' ').toLowerCase();
      return hay.includes(q);
    });
  }, [products, search]);

  function defaultComponentKind(category) {
    return category === 'storage' ? 'disk' : 'accelerator';
  }

  function openAddSpec(product) {
    const category = product.category || 'compute';
    setSpecProductId(product.id);
    setSpecProductCategory(category);
    setEditingSpec(null);
    specForm.resetFields();
    const init = {
      unit: '台',
      planned_quantity: 0,
      sort_order: 0,
      extras: [],
      components: [
        {
          kind: defaultComponentKind(category),
          model: '',
          capacity_gb: category === 'storage' ? 8000 : 0,
          qty_per_unit: 1,
          sort_order: 0,
        },
      ],
    };
    PARAM_KEYS.forEach((k) => { init[k] = ''; });
    specForm.setFieldsValue(init);
    setSpecOpen(true);
  }

  function openEditSpec(product, row) {
    const category = product.category || 'compute';
    const expectKind = defaultComponentKind(category);
    setSpecProductId(product.id);
    setSpecProductCategory(category);
    setEditingSpec(row);
    const pm = paramMap(row.parameters);
    const comps = (row.components || []).map((c, i) => ({
      kind: c.kind || expectKind,
      model: c.model || '',
      capacity_gb: c.capacity_gb ?? (category === 'storage' ? 8000 : 0),
      qty_per_unit: c.qty_per_unit ?? 1,
      sort_order: c.sort_order ?? i,
      id: c.id || '',
    }));
    const fields = {
      code: row.code || '',
      brand: row.brand || '',
      unit: row.unit || '台',
      planned_quantity: row.planned_quantity ?? 0,
      sort_order: row.sort_order ?? 0,
      extras: extraParamRows(row.parameters),
      components: comps.length
        ? comps
        : [{
            kind: expectKind,
            model: '',
            capacity_gb: category === 'storage' ? 8000 : 0,
            qty_per_unit: 1,
            sort_order: 0,
          }],
    };
    PARAM_KEYS.forEach((k) => { fields[k] = pm[k] || ''; });
    specForm.setFieldsValue(fields);
    setSpecOpen(true);
  }

  async function saveProduct() {
    try {
      const values = await productForm.validateFields();
      const payload = {
        name: values.name,
        code: values.code,
        category: values.category || 'compute',
        remark: values.remark || '',
        sort_order: Number(values.sort_order) || 0,
      };
      if (editingProduct) {
        await window.DmbitApi.put('/api/products/' + editingProduct.id, payload);
        message.success('产品已更新');
      } else {
        await window.DmbitApi.post('/api/products', payload);
        message.success('产品已创建');
      }
      setProductOpen(false);
      setEditingProduct(null);
      productForm.resetFields();
      await load();
    } catch (ex) {
      if (ex.status === 401) { window.DmbitApi.clearSession(); location.replace('/admin/login.html'); return; }
      if (ex.errorFields) return;
      message.error(ex.message || '保存失败');
    }
  }

  async function saveSpec() {
    if (!specProductId) {
      message.warning('请先选择产品');
      return;
    }
    try {
      const values = await specForm.validateFields();
      const paramValues = {};
      PARAM_KEYS.forEach((k) => { paramValues[k] = values[k]; });
      const components = (values.components || [])
        .map((c, i) => ({
          kind: c.kind || defaultComponentKind(specProductCategory),
          model: String(c.model || '').trim(),
          capacity_gb: specProductCategory === 'storage' ? Math.max(0, Number(c.capacity_gb) || 0) : 0,
          qty_per_unit: Math.max(1, Number(c.qty_per_unit) || 1),
          sort_order: Number(c.sort_order) || i,
          id: c.id || undefined,
        }))
        .filter((c) => c.model);
      const payload = {
        code: values.code,
        brand: values.brand,
        unit: values.unit || '台',
        planned_quantity: Number(values.planned_quantity) || 0,
        sort_order: Number(values.sort_order) || 0,
        parameters: buildParameters(paramValues, values.extras || []),
        product_id: specProductId,
        components,
      };
      if (editingSpec) {
        await window.DmbitApi.put('/api/specs/' + editingSpec.id, payload);
        message.success('规格已更新');
      } else {
        await window.DmbitApi.post('/api/specs', payload);
        message.success('规格已创建');
      }
      setSpecOpen(false);
      setEditingSpec(null);
      specForm.resetFields();
      await load();
    } catch (ex) {
      if (ex.status === 401) { window.DmbitApi.clearSession(); location.replace('/admin/login.html'); return; }
      if (ex.errorFields) return;
      message.error(ex.message || '保存失败');
    }
  }

  async function savePassword() {
    try {
      const values = await pwdForm.validateFields();
      await window.DmbitApi.post('/api/auth/change-password', {
        old_password: values.old_password,
        new_password: values.new_password,
      });
      message.success('密码已修改');
      setPwdOpen(false);
      pwdForm.resetFields();
    } catch (ex) {
      if (ex.status === 401) { window.DmbitApi.clearSession(); location.replace('/admin/login.html'); return; }
      if (ex.errorFields) return;
      message.error(ex.message || '修改失败');
    }
  }

  async function exportInventory() {
    setExporting(true);
    try {
      const data = await window.DmbitApi.get('/api/inventory/export');
      const csv = data && data.csv != null ? data.csv : '';
      if (!csv) { message.error('导出内容为空'); return; }
      downloadCsv(csv, '智算机房规格清单.csv');
      message.success('已导出');
    } catch (ex) {
      if (ex.status === 401) {
        window.DmbitApi.clearSession();
        location.replace('/admin/login.html');
        return;
      }
      message.error(ex.message || '导出失败');
    } finally {
      setExporting(false);
    }
  }

  async function postImport(csv, confirmUpdate) {
    return window.DmbitApi.post('/api/inventory/import', {
      csv,
      confirm_update: !!confirmUpdate,
    });
  }

  function askImportConfirm(csv, result) {
    const codes = (result.conflict_product_codes || []).join('、') || '（无）';
    const specs = (result.conflict_goods_labels || []).join('、') || '（无）';
    Modal.confirm({
      title: '检测到编号冲突',
      width: 560,
      content: React.createElement(
        'div',
        null,
        React.createElement('p', null, result.message || '以下编号已存在，继续将覆盖更新；取消则不写入任何数据。'),
        React.createElement('p', null, React.createElement('strong', null, '产品编码：'), codes),
        React.createElement('p', null, React.createElement('strong', null, '规格编码：'), specs)
      ),
      okText: '确认更新',
      cancelText: '取消',
      onOk: async () => {
        setImporting(true);
        try {
          const again = await postImport(csv, true);
          message.success((again && again.message) || '导入完成');
          await load();
        } catch (ex) {
          if (ex.status === 401) { window.DmbitApi.clearSession(); location.replace('/admin/login.html'); return; }
          message.error(ex.message || '导入失败');
          throw ex;
        } finally {
          setImporting(false);
        }
      },
    });
  }

  async function onImportFile(e) {
    const file = e.target.files && e.target.files[0];
    e.target.value = '';
    if (!file) return;
    setImporting(true);
    try {
      const csv = await file.text();
      const result = await postImport(csv, false);
      if (result && result.needs_confirm) {
        askImportConfirm(csv, result);
        return;
      }
      message.success((result && result.message) || '导入完成');
      await load();
    } catch (ex) {
      if (ex.status === 401) { window.DmbitApi.clearSession(); location.replace('/admin/login.html'); return; }
      message.error(ex.message || '导入失败');
    } finally {
      setImporting(false);
    }
  }

  const productColumns = [
    { title: '产品', dataIndex: 'name', key: 'name', ellipsis: true },
    {
      title: '类别',
      dataIndex: 'category',
      key: 'category',
      width: 88,
      render: (v) => (<Tag color={v === 'storage' ? 'purple' : 'blue'}>{categoryLabel(v)}</Tag>),
    },
    {
      title: '编码', dataIndex: 'code', key: 'code', width: 120, ellipsis: true,
    },
    {
      title: '品牌',
      key: 'brands',
      ellipsis: true,
      render: (_, row) => brandSummary(row.goods),
    },
    {
      title: '数量',
      key: 'qty',
      width: 96,
      align: 'right',
      render: (_, row) => (
        <span className="num-cell">{specTotal(row.goods).toLocaleString('zh-CN')}</span>
      ),
    },
    {
      title: '操作',
      key: 'actions',
      width: 132,
      render: (_, row) => (
        <Space size={4} onClick={(e) => e.stopPropagation()}>
          <Tooltip title="添加规格">
            <Button type="text" className="dm-icon-btn" icon={ic('add')}
              onClick={(e) => { e.stopPropagation(); openAddSpec(row); }} />
          </Tooltip>
          <Tooltip title="编辑">
            <Button type="text" className="dm-icon-btn" icon={ic('edit')}
              onClick={(e) => {
                e.stopPropagation();
                setEditingProduct(row);
                productForm.setFieldsValue({
                  name: row.name, code: row.code,
                  category: row.category || 'compute',
                  remark: row.remark || '',
                  sort_order: row.sort_order ?? 0,
                });
                setProductOpen(true);
              }} />
          </Tooltip>
          <Popconfirm title="确定删除该产品及全部规格？"
            onConfirm={async () => {
              try { await window.DmbitApi.del('/api/products/' + row.id); message.success('已删除'); await load(); }
              catch (ex) { if (ex.status === 401) { window.DmbitApi.clearSession(); location.replace('/admin/login.html'); return; } message.error(ex.message || '删除失败'); }
            }}>
            <Tooltip title="删除">
              <Button type="text" danger className="dm-icon-btn" icon={ic('delete')}
                onClick={(e) => e.stopPropagation()} />
            </Tooltip>
          </Popconfirm>
        </Space>
      ),
    },
  ];

  function specColumns(product) {
    return [
      {
        title: '规格编码', dataIndex: 'code', key: 'code', width: 130, ellipsis: true,
        render: (v) => <Text code>{v || '—'}</Text>,
      },
      { title: '品牌', dataIndex: 'brand', key: 'brand', width: 120, ellipsis: true },
      {
        title: '部件', key: 'parts', ellipsis: true, render: (_, row) => partsSummary(row),
      },
      {
        title: '数量', dataIndex: 'planned_quantity', key: 'qty', width: 88, align: 'right',
        render: (v, row) => `${Number(v || 0).toLocaleString('zh-CN')} ${row.unit || ''}`,
      },
      {
        title: '已有设备', dataIndex: 'device_count', key: 'dev', width: 88, align: 'right',
        render: (v) => (v || 0).toLocaleString('zh-CN'),
      },
      {
        title: '操作', key: 'actions', width: 96,
        render: (_, row) => (
          <Space size={4} onClick={(e) => e.stopPropagation()}>
            <Tooltip title="编辑">
              <Button type="text" className="dm-icon-btn" icon={ic('edit')}
                onClick={(e) => { e.stopPropagation(); openEditSpec(product, row); }} />
            </Tooltip>
            <Popconfirm title="确定删除该规格？"
              onConfirm={async () => {
                try { await window.DmbitApi.del('/api/specs/' + row.id); message.success('已删除'); await load(); }
                catch (ex) { if (ex.status === 401) { window.DmbitApi.clearSession(); location.replace('/admin/login.html'); return; } message.error(ex.message || '删除失败'); }
              }}>
              <Tooltip title="删除">
                <Button type="text" danger className="dm-icon-btn" icon={ic('delete')}
                  onClick={(e) => e.stopPropagation()} />
              </Tooltip>
            </Popconfirm>
          </Space>
        ),
      },
    ];
  }

  const expandedRowRender = (record) => {
    const specs = record.goods || [];
    if (!specs.length) {
      return (
        <Empty image={Empty.PRESENTED_IMAGE_SIMPLE}
          description="暂无规格，请点击「添加规格」" style={{ margin: '8px 0' }} />
      );
    }
    return (
      <Table size="small" rowKey="id" pagination={false}
        columns={specColumns(record)} dataSource={specs} scroll={{ x: 860 }} />
    );
  };

  function toggleExpand(record) {
    setExpandedKeys((keys) =>
      keys.includes(record.id) ? keys.filter((k) => k !== record.id) : [...keys, record.id]
    );
  }

  const expandIcon = ({ expanded, onExpand, record }) => (
    <button type="button" className="dm-expand-btn"
      aria-label={expanded ? '收起' : '展开'}
      onClick={(e) => { e.stopPropagation(); onExpand(record, e); }}>
      {expanded ? ic('down') : ic('right')}
    </button>
  );

  const isStorage = specProductCategory === 'storage';

  const componentColumns = (fields, remove) => {
    const cols = [
      {
        title: isStorage ? '硬盘型号' : '加速卡型号',
        render: (_, field) => (
          <>
            <Form.Item name={[field.name, 'kind']} hidden><Input /></Form.Item>
            <Form.Item name={[field.name, 'id']} hidden><Input /></Form.Item>
            <Form.Item {...field} name={[field.name, 'model']}
              rules={[{ required: true, message: '必填' }]} style={{ marginBottom: 0 }}>
              <Input placeholder={isStorage ? 'HC320' : 'RTX5090'} />
            </Form.Item>
          </>
        ),
      },
    ];
    if (isStorage) {
      cols.push({
        title: '容量', width: 120,
        render: (_, field) => (
          <Form.Item {...field} name={[field.name, 'capacity_gb']}
            rules={[{ required: true, message: '必填' },
              { validator: (_, v) => Number(v) > 0 ? Promise.resolve() : Promise.reject(new Error('须为正整数')) }]}
            style={{ marginBottom: 0 }}>
            <Input type="number" min={1} step={1} placeholder="8000(=8TB)" />
          </Form.Item>
        ),
      });
    }
    cols.push(
      {
        title: '单台数量', width: 100,
        render: (_, field) => (
          <Form.Item {...field} name={[field.name, 'qty_per_unit']}
            rules={[{ required: true, message: '必填' }]} style={{ marginBottom: 0 }}>
            <Input type="number" min={1} />
          </Form.Item>
        ),
      },
      {
        title: '', width: 48,
        render: (_, field) =>
          fields.length > 1 ? (
            <Tooltip title="删除">
              <Button type="text" danger className="dm-icon-btn" icon={ic('delete')}
                onClick={() => remove(field.name)} />
            </Tooltip>
          ) : null,
      }
    );
    return cols;
  };

  return (
    <Layout className="admin-layout">
      <Header className="admin-topbar">
        <div className="topbar-left">
          <img src="/assets/logo.svg" alt="智算机房管理" className="brand-logo" />
          <div><div className="brand-name">智算机房管理</div></div>
          <Divider type="vertical" />
          <a className="screen-link" href="/" title="打开监控大屏">
            {ic('pc')}<span>大屏</span>
          </a>
          <button type="button" className="screen-link help-link" title="使用说明"
            onClick={() => setHelpOpen(true)}>
            <span className="screen-link-icon help-icon">?</span><span>帮助</span>
          </button>
        </div>
        <div className="topbar-right">
          <Space size="middle">
            <Avatar size="small" style={{ background: 'linear-gradient(135deg,#2563eb,#7c3aed)' }}>
              {(user.name || user.email || 'A').slice(0, 1).toUpperCase()}
            </Avatar>
            <Text>{user.name || user.email || 'Admin'}</Text>
            <Button type="link" onClick={() => { pwdForm.resetFields(); setPwdOpen(true); }}>改密</Button>
            <Button type="link" onClick={() => { window.DmbitApi.clearSession(); location.replace('/admin/login.html'); }}>退出</Button>
          </Space>
        </div>
      </Header>

      <Content className="admin-content">
        <div className="toolbar-card">
          <div className="toolbar-row">
            <Space wrap size="middle">
              <Input.Search allowClear size="middle" placeholder="搜索产品名 / 编码 / 品牌 / 规格编码"
                style={{ width: 300 }} value={search} onChange={(e) => setSearch(e.target.value)} />
            </Space>
            <Space wrap>
              <input ref={importRef} type="file" accept=".csv,text/csv"
                style={{ display: 'none' }} onChange={onImportFile} />
              <Button loading={importing} icon={ic('import')}
                onClick={() => importRef.current && importRef.current.click()}>导入</Button>
              <Button loading={exporting} icon={ic('export')} onClick={exportInventory}>导出</Button>
              <Button type="primary" icon={ic('add')}
                onClick={() => {
                  setEditingProduct(null); productForm.resetFields();
                  productForm.setFieldsValue({ category: 'compute', sort_order: 0 });
                  setProductOpen(true);
                }}>新增产品</Button>
            </Space>
          </div>
        </div>

        <div className="panel-card master-table">
          <Table size="middle" rowKey="id" loading={loading}
            columns={productColumns} dataSource={filtered}
            pagination={{ pageSize: 10, showSizeChanger: false }}
            expandable={{
              expandedRowRender, expandedRowKeys: expandedKeys,
              onExpandedRowsChange: setExpandedKeys, expandIcon,
              rowExpandable: () => true,
            }}
            onRow={(record) => ({ onClick: () => toggleExpand(record), className: 'product-row' })}
            scroll={{ x: true }}
            locale={{ emptyText: <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description="暂无产品 — 先新建产品，再添加规格" /> }}
          />
        </div>
      </Content>

      {/* Product Modal */}
      <Modal title={editingProduct ? '编辑产品' : '新增产品'} open={productOpen}
        onCancel={() => setProductOpen(false)} onOk={saveProduct} destroyOnClose
        okText="保存" cancelText="取消" className="product-modal" styles={{ body: { overflowX: 'hidden' } }}>
        <Form form={productForm} layout="vertical" requiredMark={false} size="middle">
          <Row gutter={16}>
            <Col span={12}>
              <Form.Item name="name" label="名称" rules={[{ required: true, message: '必填' }]}>
                <Input placeholder="例：运算服务器" />
              </Form.Item>
            </Col>
            <Col span={12}>
              <Form.Item name="code" label="编码" rules={[{ required: true, message: '必填' }]}>
                <Input placeholder="例：CMP-SRV" />
              </Form.Item>
            </Col>
          </Row>
          <Form.Item name="category" label="类别" initialValue="compute" rules={[{ required: true }]}>
            <Select options={CATEGORY_OPTIONS} />
          </Form.Item>
          <Form.Item name="remark" label="备注">
            <Input.TextArea rows={2} placeholder="可选说明" />
          </Form.Item>
          <Form.Item name="sort_order" label="排序" initialValue={0} style={{ marginBottom: 0 }}>
            <Input type="number" />
          </Form.Item>
        </Form>
      </Modal>

      {/* Spec Modal */}
      <Modal
        title={editingSpec ? '编辑规格 · ' + (isStorage ? '存储' : '算力') : '添加规格 · ' + (isStorage ? '存储' : '算力')}
        open={specOpen} onCancel={() => setSpecOpen(false)} onOk={saveSpec} destroyOnClose
        width={740} okText="保存" cancelText="取消" className="goods-modal"
        styles={{ body: { paddingTop: 8, maxHeight: '72vh', overflowY: 'auto', overflowX: 'hidden' } }}>
        <Form form={specForm} layout="vertical" requiredMark={false} className="goods-form" size="middle">
          <div className="form-section">
            <div className="form-section-title">基本信息</div>
            <Row gutter={[16, 0]}>
              <Col span={12}>
                <Form.Item name="code" label="规格编码" rules={[{ required: true, message: '必填' }]}>
                  <Input placeholder="例：CMP-BASE" />
                </Form.Item>
              </Col>
              <Col span={12}>
                <Form.Item name="brand" label="品牌" rules={[{ required: true, message: '必填' }]}>
                  <Input placeholder="例：定制" />
                </Form.Item>
              </Col>
              <Col span={12}>
                <Form.Item name="planned_quantity" label="数量" rules={[{ required: true }]}>
                  <Input type="number" min={0} />
                </Form.Item>
              </Col>
              <Col span={12}>
                <Form.Item name="unit" label="单位" initialValue="台">
                  <Input />
                </Form.Item>
              </Col>
            </Row>
          </div>

          <div className="form-section">
            <div className="form-section-title">机箱参数</div>
            <Row gutter={[16, 0]}>
              {PARAM_KEYS.slice(0, 4).map((k) => (
                <Col span={6} key={k}>
                  <Form.Item name={k} label={k}>
                    <Input placeholder={k === '扩展' ? '6槽位PCIe' : undefined} />
                  </Form.Item>
                </Col>
              ))}
            </Row>
            <Row gutter={[16, 0]}>
              {PARAM_KEYS.slice(4).map((k) => (
                <Col span={8} key={k}>
                  <Form.Item name={k} label={k}>
                    <Input placeholder={k === '电源' ? '长城GW-800W' : undefined} />
                  </Form.Item>
                </Col>
              ))}
            </Row>
          </div>

          <div className="form-section">
            <div className="form-section-title">
              {isStorage ? '硬盘部件' : '加速卡部件'}
              <Text type="secondary" className="form-section-sub">
                {isStorage ? '型号 · 容量 · 单台块数' : '型号 · 单台张数'}
              </Text>
            </div>
            <Form.List name="components">
              {(fields, { add, remove }) => (
                <>
                  <Table size="middle" pagination={false} rowKey="key"
                    dataSource={fields} locale={{ emptyText: '暂无部件' }}
                    columns={componentColumns(fields, remove)} />
                  <Button type="dashed" block icon={ic('add')} style={{ marginTop: 8 }}
                    onClick={() => add({
                      kind: defaultComponentKind(specProductCategory),
                      model: '',
                      capacity_gb: defaultComponentKind(specProductCategory) === 'disk' ? 8000 : 0,
                      qty_per_unit: 1,
                      sort_order: fields.length,
                    })}>
                    {isStorage ? '添加硬盘' : '添加加速卡'}
                  </Button>
                </>
              )}
            </Form.List>
          </div>

          <div className="form-section form-section-muted">
            <div className="form-section-title">附加参数</div>
            <Form.List name="extras">
              {(fields, { add, remove }) => (
                <>
                  {fields.map((field) => (
                    <Row gutter={12} key={field.key} align="middle" className="extra-row">
                      <Col span={8}>
                        <Form.Item {...field} name={[field.name, 'key']} rules={[{ required: true, message: '键名' }]}>
                          <Input placeholder="参数名" />
                        </Form.Item>
                      </Col>
                      <Col span={14}>
                        <Form.Item {...field} name={[field.name, 'value']} rules={[{ required: true, message: '值' }]}>
                          <Input placeholder="参数值" />
                        </Form.Item>
                      </Col>
                      <Col span={2} style={{ textAlign: 'center' }}>
                        <Tooltip title="删除">
                          <Button type="text" danger className="dm-icon-btn" icon={ic('delete')}
                            onClick={() => remove(field.name)} />
                        </Tooltip>
                      </Col>
                    </Row>
                  ))}
                  <Button type="dashed" block icon={ic('add')}
                    onClick={() => add({ key: '', value: '' })}>添加自定义参数</Button>
                </>
              )}
            </Form.List>
          </div>

          <div className="form-section">
            <Row gutter={[16, 0]}>
              <Col span={8}>
                <Form.Item name="sort_order" label="排序" initialValue={0}>
                  <Input type="number" />
                </Form.Item>
              </Col>
            </Row>
          </div>
        </Form>
      </Modal>

      {/* Help Modal */}
      <Modal title="使用说明" open={helpOpen} onCancel={() => setHelpOpen(false)}
        footer={[<Button key="ok" type="primary" onClick={() => setHelpOpen(false)}>知道了</Button>]}
        width={760} className="help-modal" destroyOnClose>
        <div className="help-modal-body">

          <Title level={4}>一、这个系统是干什么的</Title>
          <Paragraph>
            登记机房有多少台服务器、每台配了什么卡和硬盘，然后大屏自动统计展示。
            <strong>你在本页改数据，大屏立刻看到变化。</strong>
          </Paragraph>
          <Paragraph>
            举个例子：机房里计划上架 150 台装 RTX5090 的运算服务器，每台插 6 张卡。
            你在系统里填好，大屏就会自动算出"RTX5090 共 900 张"。
          </Paragraph>

          <Title level={4}>二、三个名词（用大白话说）</Title>
          <div style={{ background: '#f0f5ff', padding: '12px 16px', borderRadius: 8, marginBottom: 12 }}>
            <p style={{ margin: '4px 0' }}><strong>产品</strong> — 服务器的大类。比如"运算服务器""存储服务器"。每个产品有一个唯一编码。</p>
            <p style={{ margin: '4px 0' }}><strong>规格</strong> — 产品下面的一种具体配置。比如运算服务器有"基础版（无显卡）""RTX5090 版""RTX4090 版"三种规格。每种规格有自己的品牌、参数、数量和部件。</p>
            <p style={{ margin: '4px 0' }}><strong>部件</strong> — 每台设备上装的卡或盘。比如 RTX5090 加速卡（每台 6 张）、DC HC320 硬盘（每台 36 块，每块 8TB）。</p>
          </div>
          <Paragraph type="danger">
            <strong>最重要的规则：</strong>显卡和硬盘信息必须填在"部件"里，大屏才能统计到。
            只在参数里写"RTX5090×6"而不建部件 = 大屏上<strong>什么都看不到</strong>。
          </Paragraph>

          <Title level={4}>三、第一次使用（跟着做就行）</Title>
          <ol style={{ lineHeight: 2 }}>
            <li>点右上角 <strong>新增产品</strong>：名称填"运算服务器"，编码填"CMP-SRV"，类别选"算力"，点保存。</li>
            <li>同样的方法再建一个"存储服务器"，编码"STO-SRV"，类别选"存储"。</li>
            <li>点击"运算服务器"那一行（展开），点 <strong>+</strong> 添加规格。</li>
            <li>规格编码填"CMP-5090"，品牌填"定制"，数量填 150，机箱参数按实际情况填写。</li>
            <li>在"加速卡部件"区域填：型号"RTX5090"，单台数量 6。点保存。</li>
            <li>同样方法再建 CMP-BASE（无显卡，数量 1800）、CMP-4090（RTX4090×6，数量 150）。</li>
            <li>给存储服务器建规格 STO-8TB：数量 388，硬盘部件填"DC HC320"、容量"8TB"、单台数量 36。</li>
            <li>打开 <a href="/" target="_blank">监控大屏</a>，检查数据是否正确。</li>
          </ol>
          <Paragraph>
            <strong>更快的方法：</strong>直接下载我们准备好的 <a href="/sample/import.csv" download>导入模板</a>，
            用 Excel 打开看看格式，然后导入即可（见下面导入教程）。
          </Paragraph>

          <Title level={4}>四、批量导入 CSV（推荐）</Title>
          <Paragraph><strong>什么时候用：</strong>设备种类多、数量大，不想一条条手工录入。</Paragraph>

          <Paragraph><strong>完整流程（5 步）：</strong></Paragraph>
          <ol style={{ lineHeight: 2 }}>
            <li><strong>导出模板：</strong>点页面上方"导出"按钮，得到一个 CSV 文件。这是当前系统里的数据。</li>
            <li><strong>用 Excel 打开编辑：</strong>双击 CSV 文件，Excel 会自动打开。每一列就是一个字段，跟表格一样。</li>
            <li><strong>修改数据：</strong>改数量、改参数、加新行。注意<strong>不要改第一行（表头）</strong>，不要删列。</li>
            <li><strong>保存：</strong>在 Excel 里点"文件 → 另存为"，格式选"CSV UTF-8（逗号分隔）"。</li>
            <li><strong>导入：</strong>点页面上方"导入"按钮，选择刚才保存的 CSV 文件。</li>
          </ol>

          <Paragraph><strong>导入时会发生什么：</strong></Paragraph>
          <ul style={{ lineHeight: 1.8 }}>
            <li>系统先检查一遍，看看有没有和现有数据冲突的编码。</li>
            <li>如果有冲突（比如"CMP-SRV"已经存在），会弹窗告诉你哪些编码冲突了。</li>
            <li>点"确认更新"会用 CSV 的内容覆盖旧数据；点"取消"则不导入任何东西。</li>
            <li>整个导入要么全部成功，要么全部不写（不会出现"改了一半"的情况）。</li>
          </ul>

          <Title level={4}>五、CSV 每一列是什么意思</Title>
          <div style={{ overflowX: 'auto' }}>
            <table style={{ width: '100%', borderCollapse: 'collapse', fontSize: 13, lineHeight: 1.8 }}>
              <thead>
                <tr style={{ background: '#f5f5f5' }}>
                  <th style={{ padding: '6px 8px', textAlign: 'left', borderBottom: '1px solid #e8e8e8' }}>列名</th>
                  <th style={{ padding: '6px 8px', textAlign: 'left', borderBottom: '1px solid #e8e8e8' }}>必填？</th>
                  <th style={{ padding: '6px 8px', textAlign: 'left', borderBottom: '1px solid #e8e8e8' }}>填什么</th>
                  <th style={{ padding: '6px 8px', textAlign: 'left', borderBottom: '1px solid #e8e8e8' }}>示例</th>
                </tr>
              </thead>
              <tbody>
                <tr><td style={c0}>产品名称</td><td style={c0}>首行必填</td><td style={c0}>服务器的大类名称</td><td style={c1}>运算服务器</td></tr>
                <tr><td style={c2}>产品编码</td><td style={c2}>必填</td><td style={c2}>唯一标识，建议英文+短横线</td><td style={c3}>CMP-SRV</td></tr>
                <tr><td style={c0}>类别</td><td style={c0}>必填</td><td style={c0}>算力 或 存储（中英文都行）</td><td style={c1}>算力</td></tr>
                <tr><td style={c2}>规格编码</td><td style={c2}>必填</td><td style={c2}>唯一标识，格式：产品缩写-特征</td><td style={c3}>CMP-5090</td></tr>
                <tr><td style={c0}>品牌</td><td style={c0}>必填</td><td style={c0}>品牌简称</td><td style={c1}>定制</td></tr>
                <tr><td style={c2}>机箱</td><td style={c2}>可选</td><td style={c2}>机箱规格描述</td><td style={c3}>4U服务器机箱</td></tr>
                <tr><td style={c0}>主板</td><td style={c0}>可选</td><td style={c0}>主板型号</td><td style={c1}>嵌入式工业级主板</td></tr>
                <tr><td style={c2}>内存</td><td style={c2}>可选</td><td style={c2}>内存规格</td><td style={c3}>SO-DIMM内存</td></tr>
                <tr><td style={c0}>接口</td><td style={c0}>可选</td><td style={c0}>IO 接口描述</td><td style={c1}>VGA、USB、以太网</td></tr>
                <tr><td style={c2}>扩展</td><td style={c2}>可选</td><td style={c2}>PCIe 扩展槽位</td><td style={c3}>6槽位PCIe扩展板</td></tr>
                <tr><td style={c0}>电源</td><td style={c0}>可选</td><td style={c0}>电源型号和功率</td><td style={c1}>定制多路输出电源</td></tr>
                <tr><td style={c2}>光模块</td><td style={c2}>可选</td><td style={c2}>网络光模块</td><td style={c3}>双光千兆</td></tr>
                <tr><td style={c0}>附加参数</td><td style={c0}>可选</td><td style={c0}>上面没列出的其他参数，用；分隔</td><td style={c1}>CPU：Intel Xeon；网卡：双口光纤</td></tr>
                <tr><td style={c2}>单位</td><td style={c2}>必填</td><td style={c2}>计量单位</td><td style={c3}>台</td></tr>
                <tr><td style={c0}><strong>数量</strong></td><td style={c0}><strong>必填</strong></td><td style={c0}>这个规格一共多少台</td><td style={c1}>150</td></tr>
                <tr><td style={c2}>部件类型</td><td style={c2}>有部件时必填</td><td style={c2}>加速卡 或 硬盘</td><td style={c3}>加速卡</td></tr>
                <tr><td style={c0}>部件型号</td><td style={c0}>有部件时必填</td><td style={c0}>具体型号</td><td style={c1}>RTX5090</td></tr>
                <tr><td style={c2}>容量</td><td style={c2}>硬盘必填</td><td style={c2}>每块硬盘的容量，如 8TB、960GB</td><td style={c3}>8TB</td></tr>
                <tr><td style={c0}>单台数量</td><td style={c0}>有部件时必填</td><td style={c0}>每台设备装几个这种部件</td><td style={c1}>6</td></tr>
                <tr><td style={c2}>备注</td><td style={c2}>可选</td><td style={c2}>任何你想备注的信息</td><td style={c3}></td></tr>
                <tr><td style={c0}>排序</td><td style={c0}>可选</td><td style={c0}>数字越小越靠前</td><td style={c1}>1</td></tr>
              </tbody>
            </table>
          </div>

          <Title level={4}>六、一个规格有多个部件怎么写</Title>
          <Paragraph>
            假设一台服务器同时装了 RTX5090 加速卡和 DC HC320 硬盘，需要写<strong>两行</strong>：
          </Paragraph>
          <div style={{ background: '#f6f8fa', padding: '12px 16px', borderRadius: 6, fontFamily: 'monospace', fontSize: 12, overflowX: 'auto', marginBottom: 12 }}>
            运算服务器,CMP-SRV,算力,CMP-HYBRID,定制,4U机箱,,,,,,,双光千兆,,台,100,加速卡,RTX5090,,6,,1<br/>
            ,CMP-SRV,,CMP-HYBRID,定制,,,,,,,,,台,100,硬盘,DC HC320,8TB,12,,
          </div>
          <Paragraph>
            <strong>规则：</strong>第一行写全部信息+第一个部件；第二行只填<strong>规格编码</strong>和<strong>部件四列</strong>，其余留空（会自动继承第一行的数据）。
          </Paragraph>

          <Title level={4}>七、常见错误和怎么改</Title>
          <table style={{ width: '100%', borderCollapse: 'collapse', fontSize: 13, lineHeight: 1.8 }}>
            <thead>
              <tr style={{ background: '#fff2f0' }}>
                <th style={{ ...c0, borderBottom: '1px solid #ffccc7' }}>错误现象</th>
                <th style={{ ...c0, borderBottom: '1px solid #ffccc7' }}>原因</th>
                <th style={{ ...c0, borderBottom: '1px solid #ffccc7' }}>怎么改</th>
              </tr>
            </thead>
            <tbody>
              <tr><td style={c0}>大屏上显卡数量是 0</td><td style={c0}>只把显卡型号写在了参数里，没有在部件列填</td><td style={c1}>在 CSV 的部件类型/部件型号/单台数量列填上显卡信息</td></tr>
              <tr><td style={c2}>导入报"表头不正确"</td><td style={c2}>改了第一行（表头），或者列的顺序变了</td><td style={c3}>重新导出一份 CSV，在导出的基础上改数据，不要动表头</td></tr>
              <tr><td style={c0}>导入报"第 N 行列数不足"</td><td style={c0}>某行少填了列，或者逗号数量不对</td><td style={c1}>用 Excel 打开检查，确保每行都有 21 个逗号分隔的值</td></tr>
              <tr><td style={c2}>导入报"加速卡不应填写容量"</td><td style={c2}>加速卡那一行的容量列填了数字</td><td style={c3}>加速卡的容量列留空，只有硬盘才填容量</td></tr>
              <tr><td style={c0}>提示"编码冲突"</td><td style={c0}>CSV 里的编码和系统里已有的重复了</td><td style={c1}>点"确认更新"会覆盖旧数据；如果是不小心写重了编码，改 CSV 里的编码再导入</td></tr>
              <tr><td style={c2}>大屏存储容量不对</td><td style={c2}>容量列填了非标准格式</td><td style={c3}>容量写"8TB"或"8000"都行（系统会自动换算），不要写"8T"或"8 TB"（中间有空格）</td></tr>
            </tbody>
          </table>

          <Title level={4}>八、大屏上能看到什么</Title>
          <ul style={{ lineHeight: 2 }}>
            <li><strong>总台数：</strong>所有规格的数量加起来</li>
            <li><strong>算力 / 存储分项：</strong>按产品类别自动分类统计</li>
            <li><strong>加速卡汇总：</strong>每种型号各多少张（规格数量 × 单台数量）</li>
            <li><strong>硬盘汇总：</strong>每种型号各多少块，以及总存储容量</li>
            <li><strong>产品构成饼图：</strong>各产品占比</li>
            <li><strong>运行状态：</strong>运行中 / 联调中 / 待上架 / 已交付 各多少台</li>
          </ul>
          <Paragraph>
            大屏每 60 秒自动刷新。你也可以手动刷新浏览器。
          </Paragraph>

          <Title level={4}>九、账号与安全</Title>
          <Paragraph>
            用管理员账号登录后，建议马上在右上角<strong>修改密码</strong>。
            密码至少 6 位。如果忘记了密码，需要联系系统管理员重置。
          </Paragraph>

        </div>
      </Modal>

      {/* Password Modal */}
      <Modal title="修改密码" open={pwdOpen} onCancel={() => setPwdOpen(false)} onOk={savePassword}
        destroyOnClose okText="确定修改" cancelText="取消" className="pwd-modal" styles={{ body: { overflowX: 'hidden' } }}>
        <Form form={pwdForm} layout="vertical" requiredMark={false} size="middle">
          <Form.Item name="old_password" label="原密码" rules={[{ required: true, message: '请输入原密码' }]}>
            <Input.Password autoComplete="current-password" />
          </Form.Item>
          <Form.Item name="new_password" label="新密码" rules={[{ required: true, message: '请输入新密码' }, { min: 6, message: '至少 6 位' }]}>
            <Input.Password autoComplete="new-password" />
          </Form.Item>
          <Form.Item name="confirm" label="确认新密码" dependencies={['new_password']}
            rules={[{ required: true, message: '请再次输入' },
              ({ getFieldValue }) => ({
                validator(_, value) {
                  if (!value || getFieldValue('new_password') === value) return Promise.resolve();
                  return Promise.reject(new Error('两次密码不一致'));
                },
              }),
            ]}>
            <Input.Password autoComplete="new-password" />
          </Form.Item>
        </Form>
      </Modal>
    </Layout>
  );
}

ReactDOM.createRoot(document.getElementById('root')).render(
  <ConfigProvider locale={zhCN}
    theme={{
      token: {
        colorPrimary: '#2563eb', colorInfo: '#7c3aed', borderRadius: 8,
        colorBgLayout: '#f5f7fb',
        fontFamily: '-apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "PingFang SC", "Microsoft YaHei", sans-serif',
      },
    }}>
    <App />
  </ConfigProvider>
);
