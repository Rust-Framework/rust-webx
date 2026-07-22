//! Seed data — roles, admin, devices from 项目需求.

use rust_ef::prelude::*;

use crate::entities::{Goods, GoodsComponent, Product, Role, RoleUser, User};
use crate::ids::seed as id;

const PARAMS_COMPUTE_CUSTOM: &str = "\
机箱：4U服务器机箱
主板：嵌入式工业级主板，配风扇的散热模块
内存：SO-DIMM内存
接口：VGA、USB、以太网等接口
扩展：6 槽位 PCIe 扩展板，搭配NVIDIA GeForce RTX显卡(不含显卡）
电源：定制多路输出电源
光模块：双光千兆";

const PARAMS_STORAGE: &str = "\
机箱：4U服务器机箱
电源：长城GW-GRPS800-2H
内存：skhynix 32GB DDR4-2133P ECC
网卡：双口光纤网卡
CPU:Intel Xeon处理器（双路）单颗核心数量28-40核，主频2.0GHZ以上，支持超线程
主板：supermicro 双路服务器主板，支撑两颗CPU及多根DDR4 ECC内存条
硬盘：8TB,DC HC320*36
光模块：双光千兆";

const PARAMS_COMPUTE_5090: &str = "\
机箱：4U服务器机箱
主板：嵌入式工业级主板，配风扇的散热模块
内存：SO-DIMM内存
接口：VGA、USB、以太网等接口
扩展：6 槽位 PCIe 扩展板
电源：定制多路输出电源
光模块：双光千兆";

const PARAMS_COMPUTE_4090: &str = "\
机箱：4U服务器机箱
主板：嵌入式工业级主板，配风扇的散热模块
内存：SO-DIMM内存
接口：VGA、USB、以太网等接口
扩展：6 槽位 PCIe 扩展板
电源：定制多路输出电源
光模块：双光千兆";

/// bcrypt hash for `admin123` (cost 4, same as docbit seed).
const ADMIN_PASSWORD_HASH: &str =
    "$2b$04$0Txv1I1N9PmPg4I9fkbZUuFVeDWIDtmlD6CEjiwxAuLzSNMHVQ/3W";

pub fn register(ctx: &mut DbContext) {
    let now = 0i64;

    ctx.model().entity::<Role>().has_data(&[Role {
        id: id::ROLE_ADMIN.into(),
        name: "admin".into(),
        description: "管理员".into(),
        created_id: None,
        created_at: now,
        updated_id: None,
        updated_at: now,
        is_deleted: false,
        users: HasMany::new(),
    }]);

    ctx.model().entity::<User>().has_data(&[User {
        id: id::USER_ADMIN.into(),
        name: "Administrator".into(),
        email: "admin@dmbit.local".into(),
        password_hash: ADMIN_PASSWORD_HASH.into(),
        created_id: None,
        created_at: now,
        updated_id: None,
        updated_at: now,
        is_deleted: false,
        roles: HasMany::new(),
    }]);

    ctx.model().entity::<RoleUser>().has_data(&[RoleUser {
        id: id::ROLE_USER_ADMIN.into(),
        user_id: id::USER_ADMIN.into(),
        role_id: id::ROLE_ADMIN.into(),
        created_at: now,
    }]);

    ctx.model().entity::<Product>().has_data(&[
        Product {
            id: id::PRODUCT_COMPUTE_CUSTOM.into(),
            name: "定制算力节点".into(),
            code: "compute-custom".into(),
            category: "compute".into(),
            remark: "智算机房定制算力（不含显卡）".into(),
            sort_order: 1,
            created_id: None,
            created_at: now,
            updated_id: None,
            updated_at: now,
            is_deleted: false,
            goods: HasMany::new(),
        },
        Product {
            id: id::PRODUCT_COMPUTE_5090.into(),
            name: "RTX 5090 算力服务器".into(),
            code: "compute-5090".into(),
            category: "compute".into(),
            remark: "".into(),
            sort_order: 2,
            created_id: None,
            created_at: now,
            updated_id: None,
            updated_at: now,
            is_deleted: false,
            goods: HasMany::new(),
        },
        Product {
            id: id::PRODUCT_COMPUTE_4090.into(),
            name: "RTX 4090 算力服务器".into(),
            code: "compute-4090".into(),
            category: "compute".into(),
            remark: "".into(),
            sort_order: 3,
            created_id: None,
            created_at: now,
            updated_id: None,
            updated_at: now,
            is_deleted: false,
            goods: HasMany::new(),
        },
        Product {
            id: id::PRODUCT_STORAGE.into(),
            name: "存储服务器".into(),
            code: "storage".into(),
            category: "storage".into(),
            remark: "智算机房存储节点（高密磁盘阵列）".into(),
            sort_order: 4,
            created_id: None,
            created_at: now,
            updated_id: None,
            updated_at: now,
            is_deleted: false,
            goods: HasMany::new(),
        },
    ]);

    ctx.model().entity::<Goods>().has_data(&[
        Goods {
            id: id::GOODS_COMPUTE_CUSTOM.into(),
            product_id: id::PRODUCT_COMPUTE_CUSTOM.into(),
            brand: "定制".into(),
            parameters: PARAMS_COMPUTE_CUSTOM.into(),
            unit: "台".into(),
            quantity: 1800,
            status: "待上架".into(),
            location: "A区 · 算力机柜群".into(),
            asset_code: "CMP-CUSTOM".into(),
            sort_order: 1,
            created_id: None,
            created_at: now,
            updated_id: None,
            updated_at: now,
            is_deleted: false,
            product: BelongsTo::new(),
        },
        Goods {
            id: id::GOODS_COMPUTE_5090.into(),
            product_id: id::PRODUCT_COMPUTE_5090.into(),
            brand: "RTX5090".into(),
            parameters: PARAMS_COMPUTE_5090.into(),
            unit: "台".into(),
            quantity: 150,
            status: "运行中".into(),
            location: "A区 · R01–R08".into(),
            asset_code: "CMP-5090".into(),
            sort_order: 2,
            created_id: None,
            created_at: now,
            updated_id: None,
            updated_at: now,
            is_deleted: false,
            product: BelongsTo::new(),
        },
        Goods {
            id: id::GOODS_COMPUTE_4090.into(),
            product_id: id::PRODUCT_COMPUTE_4090.into(),
            brand: "RTX4090".into(),
            parameters: PARAMS_COMPUTE_4090.into(),
            unit: "台".into(),
            quantity: 150,
            status: "运行中".into(),
            location: "A区 · R09–R16".into(),
            asset_code: "CMP-4090".into(),
            sort_order: 3,
            created_id: None,
            created_at: now,
            updated_id: None,
            updated_at: now,
            is_deleted: false,
            product: BelongsTo::new(),
        },
        Goods {
            id: id::GOODS_STORAGE_CUSTOM.into(),
            product_id: id::PRODUCT_STORAGE.into(),
            brand: "HC320".into(),
            parameters: PARAMS_STORAGE.into(),
            unit: "台".into(),
            quantity: 388,
            status: "联调中".into(),
            location: "B区 · 存储机柜群".into(),
            asset_code: "STO-CUSTOM".into(),
            sort_order: 1,
            created_id: None,
            created_at: now,
            updated_id: None,
            updated_at: now,
            is_deleted: false,
            product: BelongsTo::new(),
        },
    ]);

    ctx.model().entity::<GoodsComponent>().has_data(&[
        GoodsComponent {
            id: id::COMP_5090.into(),
            goods_id: id::GOODS_COMPUTE_5090.into(),
            kind: "accelerator".into(),
            model: "RTX5090".into(),
            capacity: "".into(),
            qty_per_unit: 6,
            sort_order: 1,
            created_id: None,
            created_at: now,
            updated_id: None,
            updated_at: now,
            is_deleted: false,
            goods: BelongsTo::new(),
        },
        GoodsComponent {
            id: id::COMP_4090.into(),
            goods_id: id::GOODS_COMPUTE_4090.into(),
            kind: "accelerator".into(),
            model: "RTX4090".into(),
            capacity: "".into(),
            qty_per_unit: 6,
            sort_order: 1,
            created_id: None,
            created_at: now,
            updated_id: None,
            updated_at: now,
            is_deleted: false,
            goods: BelongsTo::new(),
        },
        GoodsComponent {
            id: id::COMP_DISK_HC320.into(),
            goods_id: id::GOODS_STORAGE_CUSTOM.into(),
            kind: "disk".into(),
            model: "HC320".into(),
            capacity: "8TB".into(),
            qty_per_unit: 36,
            sort_order: 1,
            created_id: None,
            created_at: now,
            updated_id: None,
            updated_at: now,
            is_deleted: false,
            goods: BelongsTo::new(),
        },
    ]);
}
