/* global React, ReactDOM, antd, icons */
const {
  Layout, Typography, Button, Space, Table, Input, Select, Modal, Form,
  message, ConfigProvider, Avatar, Divider, Row, Col, Popconfirm, Tag, Empty,
  Tooltip,
} = antd;
const {
  PlusOutlined, EditOutlined, DeleteOutlined, ExportOutlined, ImportOutlined,
  FileAddOutlined, RightOutlined, DownOutlined, DesktopOutlined, QuestionCircleOutlined,
} = icons;
const { Header, Content } = Layout;
const { Text, Title, Paragraph } = Typography;

if (!window.DmbitApi.getToken()) {
  location.replace('/admin/login.html');
}

const STATUS_OPTIONS = [
  { value: '运行中', color: 'success' },
  { value: '联调中', color: 'warning' },
  { value: '待上架', color: 'default' },
  { value: '已交付', color: 'processing' },
];
const CATEGORY_OPTIONS = [
  { value: 'compute', label: '算力' },
  { value: 'storage', label: '存储' },
];
const PARAM_KEYS = ['机箱', '主板', '内存', '接口', '扩展', '电源', '光模块'];
const PARAM_KEY_SET = new Set(PARAM_KEYS);

const zhCN = antd.locales?.zh_CN;

function statusColor(status) {
  const hit = STATUS_OPTIONS.find((s) => s.value === status);
  return hit ? hit.color : 'default';
}

function categoryLabel(cat) {
  const hit = CATEGORY_OPTIONS.find((c) => c.value === cat);
  return hit ? hit.label : cat || '—';
}

/** Map parameters text lines key：value or key:value -> object */
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

/** Extra (non-standard) parameter rows from text */
function extraParamRows(text) {
  const map = paramMap(text);
  return Object.keys(map)
    .filter((k) => !PARAM_KEY_SET.has(k))
    .map((k) => ({ key: k, value: map[k] || '' }));
}

/** Join standard keys + extra rows into parameters text (preserves custom keys) */
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

function brandSummary(goods) {
  const brands = [...new Set((goods || []).map((g) => g.brand).filter(Boolean))];
  return brands.length ? brands.join('、') : '—';
}

function qtySum(goods) {
  return (goods || []).reduce((s, g) => s + (Number(g.quantity) || 0), 0);
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

function partsSummary(row) {
  if (row && row.parts_summary) return row.parts_summary;
  return formatComponents(row && row.components);
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
  const [statusFilter, setStatusFilter] = React.useState('');
  const [loading, setLoading] = React.useState(false);
  const [importing, setImporting] = React.useState(false);
  const [exporting, setExporting] = React.useState(false);
  const [expandedKeys, setExpandedKeys] = React.useState([]);
  const importRef = React.useRef(null);

  const [productOpen, setProductOpen] = React.useState(false);
  const [editingProduct, setEditingProduct] = React.useState(null);
  const [productForm] = Form.useForm();

  const [goodsOpen, setGoodsOpen] = React.useState(false);
  const [goodsProductId, setGoodsProductId] = React.useState(null);
  const [goodsProductCategory, setGoodsProductCategory] = React.useState('compute');
  const [editingGoods, setEditingGoods] = React.useState(null);
  const [goodsForm] = Form.useForm();

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
    return products
      .map((p) => {
        let goods = p.goods || [];
        if (statusFilter) {
          goods = goods.filter((g) => g.status === statusFilter);
        }
        return { ...p, goods };
      })
      .filter((p) => {
        if (statusFilter && !(p.goods || []).length) return false;
        if (!q) return true;
        const orig = products.find((x) => x.id === p.id) || p;
        const brands = (orig.goods || []).map((g) => g.brand || '').join(' ');
        const hay = [p.name, p.code, p.remark, p.category, brands].join(' ').toLowerCase();
        return hay.includes(q);
      });
  }, [products, search, statusFilter]);

  function defaultComponentKind(category) {
    return category === 'storage' ? 'disk' : 'accelerator';
  }

  function openAddGoods(product) {
    const category = product.category || 'compute';
    setGoodsProductId(product.id);
    setGoodsProductCategory(category);
    setEditingGoods(null);
    goodsForm.resetFields();
    const init = {
      status: '待上架',
      unit: '台',
      quantity: 0,
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
    PARAM_KEYS.forEach((k) => {
      init[k] = '';
    });
    goodsForm.setFieldsValue(init);
    setGoodsOpen(true);
  }

  function openEditGoods(product, row) {
    const category = product.category || 'compute';
    const expectKind = defaultComponentKind(category);
    setGoodsProductId(product.id);
    setGoodsProductCategory(category);
    setEditingGoods(row);
    const pm = paramMap(row.parameters);
    const comps = (row.components || [])
      .filter((c) => (c.kind || expectKind) === expectKind)
      .map((c, i) => ({
        kind: expectKind,
        model: c.model || '',
        capacity_gb: expectKind === 'disk' ? Number(c.capacity_gb) || 0 : 0,
        qty_per_unit: c.qty_per_unit ?? 1,
        sort_order: c.sort_order ?? i,
        id: c.id || '',
      }));
    const fields = {
      brand: row.brand,
      asset_code: row.asset_code || '',
      location: row.location || '',
      status: row.status || '待上架',
      unit: row.unit || '台',
      quantity: row.quantity ?? 0,
      sort_order: row.sort_order ?? 0,
      extras: extraParamRows(row.parameters),
      components: comps.length
        ? comps
        : [
            {
              kind: expectKind,
              model: '',
              capacity_gb: expectKind === 'disk' ? 8000 : 0,
              qty_per_unit: 1,
              sort_order: 0,
            },
          ],
    };
    PARAM_KEYS.forEach((k) => {
      fields[k] = pm[k] || '';
    });
    goodsForm.setFieldsValue(fields);
    setGoodsOpen(true);
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
      if (ex.errorFields) return;
      message.error(ex.message || '保存失败');
    }
  }

  async function saveGoods() {
    if (!goodsProductId) {
      message.warning('请先选择产品');
      return;
    }
    try {
      const values = await goodsForm.validateFields();
      const paramValues = {};
      PARAM_KEYS.forEach((k) => {
        paramValues[k] = values[k];
      });
      const expectKind = defaultComponentKind(goodsProductCategory);
      const components = (values.components || [])
        .map((c, i) => ({
          kind: expectKind,
          model: String(c.model || '').trim(),
          capacity_gb: expectKind === 'disk' ? Math.max(0, Number(c.capacity_gb) || 0) : 0,
          qty_per_unit: Math.max(1, Number(c.qty_per_unit) || 1),
          sort_order: Number(c.sort_order) || i,
          id: c.id || undefined,
        }))
        .filter((c) => c.model);
      const payload = {
        brand: values.brand,
        asset_code: values.asset_code || '',
        location: values.location || '',
        status: values.status,
        unit: values.unit || '台',
        quantity: Number(values.quantity) || 0,
        sort_order: Number(values.sort_order) || 0,
        parameters: buildParameters(paramValues, values.extras || []),
        product_id: goodsProductId,
        components,
      };
      if (editingGoods) {
        await window.DmbitApi.put('/api/goods/' + editingGoods.id, payload);
        message.success('台账已更新');
      } else {
        await window.DmbitApi.post('/api/goods', payload);
        message.success('台账已创建');
      }
      setGoodsOpen(false);
      setEditingGoods(null);
      goodsForm.resetFields();
      await load();
    } catch (ex) {
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
      if (ex.errorFields) return;
      message.error(ex.message || '修改失败');
    }
  }

  async function exportInventory() {
    setExporting(true);
    try {
      const data = await window.DmbitApi.get('/api/inventory/export');
      const csv = data && data.csv != null ? data.csv : '';
      if (!csv) {
        message.error('导出内容为空');
        return;
      }
      downloadCsv(csv, '智算机房台账.csv');
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
    const labels = (result.conflict_goods_labels || []).join('、') || '（无）';
    Modal.confirm({
      title: '检测到编号冲突',
      width: 560,
      content: React.createElement(
        'div',
        null,
        React.createElement('p', null, result.message || '以下编号已存在，继续将覆盖更新；取消则不写入任何数据。'),
        React.createElement('p', null, React.createElement('strong', null, '产品编码：'), codes),
        React.createElement('p', null, React.createElement('strong', null, '已有台账：'), labels)
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
          if (ex.status === 401) {
            window.DmbitApi.clearSession();
            location.replace('/admin/login.html');
            return;
          }
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
      if (ex.status === 401) {
        window.DmbitApi.clearSession();
        location.replace('/admin/login.html');
        return;
      }
      message.error(ex.message || '导入失败');
    } finally {
      setImporting(false);
    }
  }

  const productColumns = [
    { title: '产品', dataIndex: 'name', key: 'name' },
    {
      title: '类别',
      dataIndex: 'category',
      key: 'category',
      width: 88,
      render: (v) => (
        <Tag color={v === 'storage' ? 'purple' : 'blue'}>{categoryLabel(v)}</Tag>
      ),
    },
    {
      title: '品牌',
      key: 'brands',
      ellipsis: true,
      render: (_, row) => brandSummary(row.goods),
    },
    {
      title: '总数',
      key: 'qty',
      width: 88,
      align: 'right',
      render: (_, row) => (
        <span className="num-cell">{qtySum(row.goods).toLocaleString('zh-CN')}</span>
      ),
    },
    {
      title: '操作',
      key: 'actions',
      width: 132,
      render: (_, row) => (
        <Space size={4} onClick={(e) => e.stopPropagation()}>
          <Tooltip title="添加台账">
            <Button
              type="text"
              className="dm-icon-btn"
              icon={<FileAddOutlined />}
              onClick={(e) => {
                e.stopPropagation();
                openAddGoods(row);
              }}
            />
          </Tooltip>
          <Tooltip title="编辑">
            <Button
              type="text"
              className="dm-icon-btn"
              icon={<EditOutlined />}
              onClick={(e) => {
                e.stopPropagation();
                setEditingProduct(row);
                productForm.setFieldsValue({
                  name: row.name,
                  code: row.code,
                  category: row.category || 'compute',
                  remark: row.remark || '',
                  sort_order: row.sort_order ?? 0,
                });
                setProductOpen(true);
              }}
            />
          </Tooltip>
          <Popconfirm
            title="确定删除该产品及全部台账？"
            onConfirm={async () => {
              try {
                await window.DmbitApi.del('/api/products/' + row.id);
                message.success('已删除');
                await load();
              } catch (ex) {
                message.error(ex.message || '删除失败');
              }
            }}
          >
            <Tooltip title="删除">
              <Button
                type="text"
                danger
                className="dm-icon-btn"
                icon={<DeleteOutlined />}
                onClick={(e) => e.stopPropagation()}
              />
            </Tooltip>
          </Popconfirm>
        </Space>
      ),
    },
  ];

  function goodsColumns(product) {
    return [
      { title: '品牌', dataIndex: 'brand', key: 'brand', width: 120, ellipsis: true },
      {
        title: '状态',
        dataIndex: 'status',
        key: 'status',
        width: 96,
        render: (v) => <Tag color={statusColor(v)}>{v || '—'}</Tag>,
      },
      {
        title: '部件',
        key: 'parts',
        ellipsis: true,
        render: (_, row) => partsSummary(row),
      },
      {
        title: '数量',
        dataIndex: 'quantity',
        key: 'quantity',
        width: 88,
        align: 'right',
        render: (v, row) =>
          `${Number(v || 0).toLocaleString('zh-CN')} ${row.unit || ''}`,
      },
      {
        title: '机位',
        dataIndex: 'location',
        key: 'location',
        width: 160,
        ellipsis: true,
        render: (v) => v || '—',
      },
      {
        title: '操作',
        key: 'actions',
        width: 96,
        render: (_, row) => (
          <Space size={4} onClick={(e) => e.stopPropagation()}>
            <Tooltip title="编辑">
              <Button
                type="text"
                className="dm-icon-btn"
                icon={<EditOutlined />}
                onClick={(e) => {
                  e.stopPropagation();
                  openEditGoods(product, row);
                }}
              />
            </Tooltip>
            <Popconfirm
              title="确定删除该台账行？"
              onConfirm={async () => {
                try {
                  await window.DmbitApi.del('/api/goods/' + row.id);
                  message.success('已删除');
                  await load();
                } catch (ex) {
                  message.error(ex.message || '删除失败');
                }
              }}
            >
              <Tooltip title="删除">
                <Button
                  type="text"
                  danger
                  className="dm-icon-btn"
                  icon={<DeleteOutlined />}
                  onClick={(e) => e.stopPropagation()}
                />
              </Tooltip>
            </Popconfirm>
          </Space>
        ),
      },
    ];
  }

  const expandedRowRender = (record) => {
    const goods = record.goods || [];
    if (!goods.length) {
      return (
        <Empty
          image={Empty.PRESENTED_IMAGE_SIMPLE}
          description="暂无台账，请点击「添加台账」"
          style={{ margin: '8px 0' }}
        />
      );
    }
    return (
      <Table
        size="small"
        rowKey="id"
        pagination={false}
        columns={goodsColumns(record)}
        dataSource={goods}
        scroll={{ x: 860 }}
      />
    );
  };

  function toggleExpand(record) {
    setExpandedKeys((keys) =>
      keys.includes(record.id) ? keys.filter((k) => k !== record.id) : [...keys, record.id]
    );
  }

  const expandIcon = ({ expanded, onExpand, record }) => (
    <button
      type="button"
      className="dm-expand-btn"
      aria-label={expanded ? '收起' : '展开'}
      onClick={(e) => {
        e.stopPropagation();
        onExpand(record, e);
      }}
    >
      {expanded ? <DownOutlined /> : <RightOutlined />}
    </button>
  );

  const isStorage = goodsProductCategory === 'storage';

  const componentColumns = (fields, remove) => {
    const cols = [
      {
        title: isStorage ? '硬盘型号' : '加速卡型号',
        render: (_, field) => (
          <>
            <Form.Item name={[field.name, 'kind']} hidden>
              <Input />
            </Form.Item>
            <Form.Item name={[field.name, 'id']} hidden>
              <Input />
            </Form.Item>
            <Form.Item
              {...field}
              name={[field.name, 'model']}
              rules={[{ required: true, message: '必填' }]}
              style={{ marginBottom: 0 }}
            >
              <Input placeholder={isStorage ? 'HC320' : 'RTX5090'} />
            </Form.Item>
          </>
        ),
      },
    ];
    if (isStorage) {
      cols.push({
        title: '容量(GB)',
        width: 120,
        render: (_, field) => (
          <Form.Item
            {...field}
            name={[field.name, 'capacity_gb']}
            rules={[
              { required: true, message: '必填' },
              {
                validator: (_, v) =>
                  Number(v) > 0
                    ? Promise.resolve()
                    : Promise.reject(new Error('须为正整数 GB')),
              },
            ]}
            style={{ marginBottom: 0 }}
          >
            <Input type="number" min={1} step={1} placeholder="8000(=8TB)" />
          </Form.Item>
        ),
      });
    }
    cols.push(
      {
        title: '单台数量',
        width: 100,
        render: (_, field) => (
          <Form.Item
            {...field}
            name={[field.name, 'qty_per_unit']}
            rules={[{ required: true, message: '必填' }]}
            style={{ marginBottom: 0 }}
          >
            <Input type="number" min={1} />
          </Form.Item>
        ),
      },
      {
        title: '',
        width: 48,
        render: (_, field) =>
          fields.length > 1 ? (
            <Tooltip title="删除">
              <Button
                type="text"
                danger
                className="dm-icon-btn"
                icon={<DeleteOutlined />}
                onClick={() => remove(field.name)}
              />
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
          <div>
            <div className="brand-name">智算机房管理</div>
          </div>
          <Divider type="vertical" />
          <a className="screen-link" href="/" title="打开监控大屏">
            <DesktopOutlined className="screen-link-icon" />
            <span>大屏</span>
          </a>
          <button
            type="button"
            className="screen-link help-link"
            title="使用说明"
            onClick={() => setHelpOpen(true)}
          >
            <QuestionCircleOutlined className="screen-link-icon" />
            <span>帮助</span>
          </button>
        </div>
        <div className="topbar-right">
          <Space size="middle">
            <Avatar size="small" style={{ background: 'linear-gradient(135deg,#2563eb,#7c3aed)' }}>
              {(user.name || user.email || 'A').slice(0, 1).toUpperCase()}
            </Avatar>
            <Text>{user.name || user.email || 'Admin'}</Text>
            <Button
              type="link"
              onClick={() => {
                pwdForm.resetFields();
                setPwdOpen(true);
              }}
            >
              改密
            </Button>
            <Button
              type="link"
              onClick={() => {
                window.DmbitApi.clearSession();
                location.replace('/admin/login.html');
              }}
            >
              退出
            </Button>
          </Space>
        </div>
      </Header>

      <Content className="admin-content">
        <div className="toolbar-card">
          <div className="toolbar-row">
            <Space wrap size="middle">
              <Input.Search
                allowClear
                size="middle"
                placeholder="搜索产品名 / 编码 / 品牌短码"
                style={{ width: 300 }}
                value={search}
                onChange={(e) => setSearch(e.target.value)}
              />
              <Select
                allowClear
                size="middle"
                placeholder="按状态筛选"
                style={{ width: 140 }}
                value={statusFilter || undefined}
                onChange={(v) => setStatusFilter(v || '')}
                options={STATUS_OPTIONS.map((s) => ({ value: s.value, label: s.value }))}
              />
            </Space>
            <Space wrap>
              <input
                ref={importRef}
                type="file"
                accept=".csv,text/csv"
                style={{ display: 'none' }}
                onChange={onImportFile}
              />
              <Button
                loading={importing}
                icon={<ImportOutlined />}
                onClick={() => importRef.current && importRef.current.click()}
              >
                导入
              </Button>
              <Button loading={exporting} icon={<ExportOutlined />} onClick={exportInventory}>
                导出
              </Button>
              <Button
                type="primary"
                icon={<PlusOutlined />}
                onClick={() => {
                  setEditingProduct(null);
                  productForm.resetFields();
                  productForm.setFieldsValue({ category: 'compute', sort_order: 0 });
                  setProductOpen(true);
                }}
              >
                新增产品
              </Button>
            </Space>
          </div>
        </div>

        <div className="panel-card master-table">
          <Table
            size="middle"
            rowKey="id"
            loading={loading}
            columns={productColumns}
            dataSource={filtered}
            pagination={{ pageSize: 10, showSizeChanger: false }}
            expandable={{
              expandedRowRender,
              expandedRowKeys: expandedKeys,
              onExpandedRowsChange: setExpandedKeys,
              expandIcon,
              rowExpandable: () => true,
            }}
            onRow={(record) => ({
              onClick: () => toggleExpand(record),
              className: 'product-row',
            })}
            scroll={{ x: true }}
            locale={{
              emptyText: (
                <Empty
                  image={Empty.PRESENTED_IMAGE_SIMPLE}
                  description="暂无产品 — 先新建产品，再添加台账"
                />
              ),
            }}
          />
        </div>
      </Content>

      <Modal
        title={editingProduct ? '编辑产品' : '新增产品'}
        open={productOpen}
        onCancel={() => setProductOpen(false)}
        onOk={saveProduct}
        destroyOnClose
        okText="保存"
        cancelText="取消"
        className="product-modal"
        styles={{ body: { overflowX: 'hidden' } }}
      >
        <Form form={productForm} layout="vertical" requiredMark={false} size="middle">
          <Form.Item name="name" label="名称" rules={[{ required: true, message: '必填' }]}>
            <Input placeholder="例：RTX 5090 算力服务器" />
          </Form.Item>
          <Form.Item name="code" label="编码" rules={[{ required: true, message: '必填' }]}>
            <Input placeholder="例：compute-5090" />
          </Form.Item>
          <Form.Item
            name="category"
            label="类别"
            initialValue="compute"
            rules={[{ required: true, message: '必填' }]}
          >
            <Select options={CATEGORY_OPTIONS} />
          </Form.Item>
          <Form.Item name="remark" label="备注">
            <Input.TextArea rows={2} placeholder="可选说明" />
          </Form.Item>
          <Form.Item name="sort_order" label="排序" initialValue={0}>
            <Input type="number" />
          </Form.Item>
        </Form>
      </Modal>

      <Modal
        title={
          editingGoods
            ? '编辑台账 · ' + (isStorage ? '存储' : '算力')
            : '添加台账 · ' + (isStorage ? '存储' : '算力')
        }
        open={goodsOpen}
        onCancel={() => setGoodsOpen(false)}
        onOk={saveGoods}
        destroyOnClose
        width={740}
        okText="保存"
        cancelText="取消"
        className="goods-modal"
        styles={{ body: { paddingTop: 8, maxHeight: '72vh', overflowY: 'auto', overflowX: 'hidden' } }}
      >
        <Form
          form={goodsForm}
          layout="vertical"
          requiredMark={false}
          className="goods-form"
          size="middle"
        >
          <div className="form-section">
            <div className="form-section-title">身份与数量</div>
            <Row gutter={[16, 0]}>
              <Col span={12}>
                <Form.Item
                  name="brand"
                  label="品牌短码"
                  rules={[{ required: true, message: '必填' }]}
                >
                  <Input placeholder={isStorage ? 'HC320' : 'RTX5090'} />
                </Form.Item>
              </Col>
              <Col span={12}>
                <Form.Item name="status" label="状态" rules={[{ required: true }]}>
                  <Select options={STATUS_OPTIONS.map((s) => ({ value: s.value, label: s.value }))} />
                </Form.Item>
              </Col>
              <Col span={8}>
                <Form.Item name="quantity" label="数量" rules={[{ required: true, message: '必填' }]}>
                  <Input type="number" min={0} />
                </Form.Item>
              </Col>
              <Col span={8}>
                <Form.Item name="unit" label="单位" initialValue="台">
                  <Input />
                </Form.Item>
              </Col>
              <Col span={8}>
                <Form.Item name="sort_order" label="排序" initialValue={0}>
                  <Input type="number" />
                </Form.Item>
              </Col>
            </Row>
          </div>

          <div className="form-section">
            <div className="form-section-title">位置与资产</div>
            <Row gutter={[16, 0]}>
              <Col span={12}>
                <Form.Item name="location" label="机位">
                  <Input placeholder="例：A区 · R01–R08" />
                </Form.Item>
              </Col>
              <Col span={12}>
                <Form.Item name="asset_code" label="资产编码">
                  <Input placeholder="例：CMP-5090" />
                </Form.Item>
              </Col>
            </Row>
          </div>

          <div className="form-section">
            <div className="form-section-title">机箱参数</div>
            <Row gutter={[16, 0]}>
              {PARAM_KEYS.map((k) => (
                <Col span={8} key={k}>
                  <Form.Item name={k} label={k}>
                    <Input placeholder={k === '扩展' ? 'PCIe×6' : undefined} />
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
                  <Table
                    size="middle"
                    pagination={false}
                    rowKey="key"
                    dataSource={fields}
                    locale={{ emptyText: '暂无部件' }}
                    columns={componentColumns(fields, remove)}
                  />
                  <Button
                    type="dashed"
                    block
                    icon={<PlusOutlined />}
                    style={{ marginTop: 8 }}
                    onClick={() =>
                      add({
                        kind: defaultComponentKind(goodsProductCategory),
                        model: '',
                        capacity_gb:
                          defaultComponentKind(goodsProductCategory) === 'disk' ? 8000 : 0,
                        qty_per_unit: 1,
                        sort_order: fields.length,
                      })
                    }
                  >
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
                        <Form.Item
                          {...field}
                          name={[field.name, 'key']}
                          rules={[{ required: true, message: '键名' }]}
                        >
                          <Input placeholder="参数名" />
                        </Form.Item>
                      </Col>
                      <Col span={14}>
                        <Form.Item
                          {...field}
                          name={[field.name, 'value']}
                          rules={[{ required: true, message: '值' }]}
                        >
                          <Input placeholder="参数值" />
                        </Form.Item>
                      </Col>
                      <Col span={2} style={{ textAlign: 'center' }}>
                        <Tooltip title="删除">
                          <Button
                            type="text"
                            danger
                            className="dm-icon-btn"
                            icon={<DeleteOutlined />}
                            onClick={() => remove(field.name)}
                          />
                        </Tooltip>
                      </Col>
                    </Row>
                  ))}
                  <Button type="dashed" block icon={<PlusOutlined />} onClick={() => add({ key: '', value: '' })}>
                    添加自定义参数
                  </Button>
                </>
              )}
            </Form.List>
          </div>
        </Form>
      </Modal>

      <Modal
        title="使用说明"
        open={helpOpen}
        onCancel={() => setHelpOpen(false)}
        footer={[
          <Button key="ok" type="primary" onClick={() => setHelpOpen(false)}>
            知道了
          </Button>,
        ]}
        width={720}
        className="help-modal"
        destroyOnClose
      >
        <div className="help-modal-body">
          <Title level={5}>这个系统做什么</Title>
          <Paragraph>
            <strong>后台</strong>（本页）登记机房有哪些整机、各多少台、每台挂几张卡/几块盘；
            <strong>大屏</strong>（首页）自动汇总展示。改完后台刷新大屏即可看到变化。
          </Paragraph>

          <Title level={5}>三个名词（务必分清）</Title>
          <ul>
            <li>
              <strong>产品</strong>：整机型号（名称、唯一编码、类别：算力或存储）。
            </li>
            <li>
              <strong>台账</strong>：该型号下的一批机器（品牌短码、台数、状态、机位、资产编码、机箱参数等）。
            </li>
            <li>
              <strong>部件</strong>：每台机器上的加速卡或硬盘（型号 + 单台数量；硬盘还要填容量）。
            </li>
          </ul>
          <Paragraph>
            <strong>重要：</strong>大屏上的卡数、盘数只统计「部件」表。
            只把「RTX5090×6」写在扩展说明里、不建部件 → 大屏<strong>统计不到</strong>。
            公式：卡/盘总数 = 台账台数 × 该部件「单台数量」。
          </Paragraph>

          <Title level={5}>第一次怎么用（手工登记）</Title>
          <ol>
            <li>
              右上角<strong>新增产品</strong>：填名称、编码、选类别（算力 / 存储）。
            </li>
            <li>
              在该产品行点<strong>添加台账</strong>：填品牌短码、数量、状态、机位等。
            </li>
            <li>
              在台账表单里维护<strong>部件</strong>：算力填加速卡（如型号 RTX5090、单台 6）；
              存储填硬盘（型号、容量 GB 如 8000=8TB、单台块数）。保存后即整单替换该台账的部件列表。
            </li>
            <li>
              点击产品行可展开，查看已有台账与部件摘要；需要时再点编辑。
            </li>
          </ol>
          <Paragraph>
            <strong>示例：</strong>新增「昇腾算力服务器」→ 类别选算力 → 添加台账数量 80 →
            部件填 Ascend910B、单台 8 → 大屏加速卡会出现 Ascend910B，张数 = 80×8。
          </Paragraph>

          <Title level={5}>批量导入 / 导出</Title>
          <Paragraph>
            推荐流程：先<strong>导出</strong> → Excel 另存为 CSV（UTF-8）→ 改完再<strong>导入</strong>。
            表头与导出一致，勿改列名。
          </Paragraph>
          <ul>
            <li>
              同一台账：<strong>首行</strong>写满产品/台账字段（可带第一个部件）；
              <strong>续行</strong>只填四键（产品编码、品牌短码、资产编码、机位）+ 部件列，其余留空。
            </li>
            <li>
              续行若误填状态、数量等且与首行不同，导入会报错并指出行号。
            </li>
            <li>
              该台账任一行填了部件列 → 重建部件；部件列全空 → <strong>不改动</strong>库里已有部件。
            </li>
            <li>
              若产品编码或台账（产品 + 品牌 + 资产编码 + 机位）已存在，会先列出冲突；
              确认后才覆盖，取消则保持原样。整次导入一次提交。
            </li>
          </ul>

          <Title level={5}>大屏上看什么</Title>
          <ul>
            <li>设备总台数，以及算力 / 存储分项（来自台账数量 × 产品类别）</li>
            <li>加速卡按型号张数；硬盘按「型号 · 容量」块数（来自部件）</li>
            <li>
              存储容量：各硬盘部件「容量(GB) × 块数」整数汇总；CSV 里可写 8TB，入库时换算为 GB
            </li>
            <li>产品构成、运行状态分布</li>
          </ul>

          <Title level={5}>账号与安全</Title>
          <Paragraph>
            使用部署时提供的管理员账号登录；首次登录后请尽快在右上角<strong>修改密码</strong>。
            请勿在公共场合展示口令。
          </Paragraph>
        </div>
      </Modal>

      <Modal
        title="修改密码"
        open={pwdOpen}
        onCancel={() => setPwdOpen(false)}
        onOk={savePassword}
        destroyOnClose
        okText="确定修改"
        cancelText="取消"
        className="pwd-modal"
        styles={{ body: { overflowX: 'hidden' } }}
      >
        <Form form={pwdForm} layout="vertical" requiredMark={false} size="middle">
          <Form.Item
            name="old_password"
            label="原密码"
            rules={[{ required: true, message: '请输入原密码' }]}
          >
            <Input.Password autoComplete="current-password" />
          </Form.Item>
          <Form.Item
            name="new_password"
            label="新密码"
            rules={[
              { required: true, message: '请输入新密码' },
              { min: 6, message: '至少 6 位' },
            ]}
          >
            <Input.Password autoComplete="new-password" />
          </Form.Item>
          <Form.Item
            name="confirm"
            label="确认新密码"
            dependencies={['new_password']}
            rules={[
              { required: true, message: '请再次输入' },
              ({ getFieldValue }) => ({
                validator(_, value) {
                  if (!value || getFieldValue('new_password') === value) {
                    return Promise.resolve();
                  }
                  return Promise.reject(new Error('两次密码不一致'));
                },
              }),
            ]}
          >
            <Input.Password autoComplete="new-password" />
          </Form.Item>
        </Form>
      </Modal>
    </Layout>
  );
}

ReactDOM.createRoot(document.getElementById('root')).render(
  <ConfigProvider
    locale={zhCN}
    theme={{
      token: {
        colorPrimary: '#2563eb',
        colorInfo: '#7c3aed',
        borderRadius: 8,
        colorBgLayout: '#f5f7fb',
        fontFamily:
          '-apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "PingFang SC", "Microsoft YaHei", sans-serif',
      },
    }}
  >
    <App />
  </ConfigProvider>
);
