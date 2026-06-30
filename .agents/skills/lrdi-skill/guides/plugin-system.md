# LRDI 插件系统深度指南

## 一、插件系统的三种架构层次

LRDI 提供三层插件支持，从简单到复杂：

### 层次 1：键控服务（进程内插件）
同一个进程内，用 keyed 服务实现策略/插件切换。
适用：固定数量的实现变体，编译时已知。

### 层次 2：分层容器（进程内隔离）
用 ServiceProviderWrapper 为每个插件创建独立容器。
适用：需要服务隔离、服务覆盖、模块化架构。

### 层次 3：命名服务 + IServiceLocator（跨 DLL 插件）
用命名服务跨 cdylib 边界共享服务。
适用：运行时热加载/卸载、第三方插件 SDK。

## 二、层次 1：键控服务详解

完整示例：CLI 工具的命令插件系统。
- 定义 Command trait（name, execute, help）
- HelloCommand, TimeCommand, ExitCommand 实现
- 注册为 keyed::<dyn Command>
- 命令路由：get_keyed(user_input)
- 动态发现：get_all::<dyn Command>() 列出所有命令

## 三、层次 2：分层容器详解

### 3.1 架构原理
ServiceProviderWrapper 提供 child-first, root-fallback 解析。
子容器（child）的注册优先于根容器（root）。
子容器解析失败时 fallback 到根容器。

### 3.2 关键行为演示
通过测试代码展示三个关键行为：
1. child 优先：child 和 root 都注册了同类型 B，解析得到 child 的实例
2. root fallback：child 没注册 RO，解析 fallback 到 root
3. child 不可见：root 无法看到 child 注册的 PO

### 3.3 实战：微内核架构
完整的端到端示例：
- 宿主（root provider）：EventBus, ConfigService, Logger（Singleton）
- 插件 A：自己的服务 + 覆盖 Logger（child provider）
- 插件 B：自己的服务
- PluginManager：管理插件加载/卸载，每个插件创建 Wrapper
- 插件之间通过 EventBus（root 中的服务）通信

讲解：
- 为什么插件能覆盖宿主服务（child-first）
- 为什么插件不能访问其他插件的服务（各自独立 child）
- 为什么插件能访问 EventBus（root fallback）

## 四、层次 3：跨 DLL 插件详解

### 4.1 问题：TypeId 在 cdylib 中不稳定
Rust 为每个编译单元分配不同的 TypeId。同一个类型在宿主和插件中有不同的 TypeId，因此 TypeId-based 解析（正常的 get::<T>()）跨 DLL 必然失败。

### 4.2 解决方案：命名服务
- register_named(name, Arc<T>)：以字符串 key 存储服务
- get_named::<T>(name)：以字符串 key 查询 + downcast

内部实现：ServiceProvider 持有 named: RwLock<HashMap<String, Arc<dyn Any + Send + Sync>>>

### 4.3 完整示例：插件热加载系统
提供完整的端到端代码：
1. 定义共享接口 crate (plugin_api)：
   - trait IPlugin: Send + Sync { fn name(); fn init(locator: Arc<dyn IServiceLocator>); fn execute(); fn shutdown(); }
   - trait IPluginHost: Send + Sync { fn register_named(); fn get_named(); }

2. 宿主端：
   - 构建 ServiceProvider，注册 EventBus、Config 等
   - 实现 IPluginHost（包装 ServiceProvider 的 named 操作）
   - 从文件系统加载 .so/.dll 文件
   - 调用插件的 init，传入 IServiceLocator

3. 插件端：
   - 导出 extern "C" fn create_plugin() -> *mut dyn IPlugin
   - 在 init 中通过 IServiceLocator 获取宿主服务
   - 注册插件自己的服务到 named registry

4. 卸载流程：
   - 调用 shutdown()
   - remove_named() 移除插件注册的服务
   - 卸载动态库

### 4.4 安全性考虑
- 类型安全：get_named downcast 失败返回 None，不 panic
- 内存安全：Arc 确保服务在插件卸载后仍然有效（如果宿主持有引用）
- 线程安全：named registry 使用 RwLock
- 悬垂指针：插件卸载时由 Arc 引用计数保证安全

### 4.5 IServiceLocator 接口详解
IServiceLocator trait 的四个方法：
- get_any(type_key: &str)：通过类型名解析（类型擦除）
- get_any_named(name: &str)：通过名称解析（命名服务）
- register_named_any(name, svc)：注册命名服务
- remove_named(name)：移除命名服务

ServiceLocatorBridge 实现：包装 RdiProvider（Root 或 Wrapped）

## 五、插件模式总结对比

| 特性 | 键控服务 | 分层容器 | 跨 DLL 插件 |
|------|---------|---------|------------|
| 隔离级别 | 无隔离 | 类型隔离 | 编译单元隔离 |
| 动态加载 | 否 | 是（可重建容器） | 是 |
| 服务覆盖 | 否 | 是（child-first） | 通过命名服务 |
| 跨语言 | 否 | 否 | 通过 FFI |
| 复杂度 | 低 | 中 | 高 |
| 性能开销 | 最小 | 中等（Wrapper 层） | 有（downcast + FFI） |
