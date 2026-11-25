//! Device ↔ `DeviceModel` mapping.

use dmbit_contracts::device::{CreateDeviceRequest, DeviceModel, UpdateDeviceRequest};
use rust_ef::prelude::*;
use webx_core::{Error, IPatchable, Result};

use crate::entities::Device;
use crate::ids::new_id;
use crate::mappers::{now_secs, operator_id};
use crate::rules::{
    normalize_status, optional_text, require_text, MAX_ASSET_CODE, MAX_LOCATION, MAX_SERIAL_NO,
};

/// Device row + the joined spec code / product name.
pub fn device_model(d: &Device, spec_code: &str, product_name: &str) -> DeviceModel {
    DeviceModel {
        id: d.id.clone(),
        spec_id: d.spec_id.clone(),
        spec_code: spec_code.to_string(),
        product_name: product_name.to_string(),
        status: d.status.clone(),
        location: d.location.clone(),
        asset_code: d.asset_code.clone(),
        serial_no: d.serial_no.clone(),
        sort_order: d.sort_order,
        created_at: d.created_at,
        updated_at: d.updated_at,
    }
}

impl TryFrom<CreateDeviceRequest> for Device {
    type Error = Error;

    fn try_from(req: CreateDeviceRequest) -> Result<Self> {
        let spec_id = req.spec_id.trim();
        if spec_id.is_empty() {
            return Err(Error::validation("标识不能为空"));
        }
        if spec_id.chars().count() > 36 {
            return Err(Error::validation("标识格式无效"));
        }
        let status = normalize_status(&req.status)?;
        let location = optional_text("机位", &req.location, MAX_LOCATION)?;
        let asset_code = require_text("资产编码", &req.asset_code, MAX_ASSET_CODE)?;
        let serial_no = optional_text("序列号", &req.serial_no, MAX_SERIAL_NO)?;

        let now = now_secs();
        let actor = operator_id();
        Ok(Device {
            id: new_id(),
            spec_id: spec_id.to_string(),
            status,
            location,
            asset_code,
            serial_no,
            sort_order: req.sort_order,
            created_id: actor.clone(),
            created_at: now,
            updated_id: actor,
            updated_at: now,
            is_deleted: false,
            spec: BelongsTo::new(),
        })
    }
}

/// Validate and normalize a partial update before [`IPatchable::patch`].
///
/// `spec_id` is left untouched: moving a device needs the context to prove the
/// target spec exists, so the handler resolves it.
pub fn normalize_update_device(req: &mut UpdateDeviceRequest) -> Result<()> {
    if let Some(v) = req.status.take() {
        req.status = Some(normalize_status(&v)?);
    }
    if let Some(v) = req.location.take() {
        req.location = Some(optional_text("机位", &v, MAX_LOCATION)?);
    }
    if let Some(v) = req.asset_code.take() {
        req.asset_code = Some(require_text("资产编码", &v, MAX_ASSET_CODE)?);
    }
    if let Some(v) = req.serial_no.take() {
        req.serial_no = Some(optional_text("序列号", &v, MAX_SERIAL_NO)?);
    }
    Ok(())
}

impl IPatchable<UpdateDeviceRequest> for Device {
    fn patch(&mut self, patch: UpdateDeviceRequest) {
        if let Some(v) = patch.status {
            self.status = v;
        }
        if let Some(v) = patch.location {
            self.location = v;
        }
        if let Some(v) = patch.asset_code {
            self.asset_code = v;
        }
        if let Some(v) = patch.serial_no {
            self.serial_no = v;
        }
        if let Some(v) = patch.sort_order {
            self.sort_order = v;
        }
    }
}

/// Server-trusted values for a new device row.
pub struct DeviceSeed {
    pub id: String,
    pub spec_id: String,
    pub status: String,
    pub location: String,
    pub asset_code: String,
    pub serial_no: String,
    pub sort_order: i32,
    pub actor: Option<String>,
    pub now: i64,
}

impl From<DeviceSeed> for Device {
    fn from(seed: DeviceSeed) -> Self {
        Device {
            id: seed.id,
            spec_id: seed.spec_id,
            status: seed.status,
            location: seed.location,
            asset_code: seed.asset_code,
            serial_no: seed.serial_no,
            sort_order: seed.sort_order,
            created_id: seed.actor.clone(),
            created_at: seed.now,
            updated_id: seed.actor,
            updated_at: seed.now,
            is_deleted: false,
            spec: BelongsTo::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device() -> Device {
        Device {
            id: "d1".into(),
            spec_id: "s1".into(),
            status: "运行中".into(),
            location: "A区".into(),
            asset_code: "A-1".into(),
            serial_no: String::new(),
            sort_order: 1,
            created_id: None,
            created_at: 0,
            updated_id: None,
            updated_at: 0,
            is_deleted: false,
            spec: BelongsTo::new(),
        }
    }

    fn update() -> UpdateDeviceRequest {
        UpdateDeviceRequest {
            id: "d1".into(),
            ..Default::default()
        }
    }

    #[test]
    fn create_request_is_validated_and_server_fields_are_assigned() {
        let entity = Device::try_from(CreateDeviceRequest {
            spec_id: " s1 ".into(),
            status: " 已交付 ".into(),
            asset_code: " A-9 ".into(),
            ..Default::default()
        })
        .expect("valid device");
        assert_eq!(entity.spec_id, "s1");
        assert_eq!(entity.status, "已交付");
        assert_eq!(entity.asset_code, "A-9");
        assert!(!entity.id.is_empty());

        assert!(Device::try_from(CreateDeviceRequest {
            spec_id: "s1".into(),
            status: "报废".into(),
            asset_code: "A-2".into(),
            ..Default::default()
        })
        .is_err());
    }

    #[test]
    fn normalize_then_patch_overwrites_only_the_fields_it_carries() {
        let mut patch = update();
        patch.status = Some(" 联调中 ".into());
        patch.sort_order = Some(7);
        normalize_update_device(&mut patch).expect("valid patch");

        let mut entity = device();
        entity.patch(patch);
        assert_eq!(entity.status, "联调中");
        assert_eq!(entity.sort_order, 7);
        assert_eq!(entity.location, "A区", "absent location is untouched");

        let mut invalid = update();
        invalid.status = Some("报废".into());
        assert!(normalize_update_device(&mut invalid).is_err());
    }
}
